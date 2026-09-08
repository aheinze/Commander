use sha2::{Digest, Sha256};

use super::*;

pub(crate) fn download(
    update: &Available,
    asset: &Asset,
    destination: &Path,
    cancel: &CancelToken,
    progress: impl FnMut(u64),
) -> Result<PathBuf> {
    check_cancel(cancel)?;
    let response = github::agent(600)
        .get(update.download_url(asset))
        .header(
            "User-Agent",
            concat!("Commander/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|_| {
            Failure::new("Could not download the update. Check your connection and try again.")
        })?;
    if response.status().as_u16() != 200 {
        return Err(github::response_failure(response.status().as_u16(), None));
    }
    receive(
        response.into_body().into_reader(),
        asset,
        destination,
        cancel,
        progress,
    )
}

pub(super) fn receive(
    mut reader: impl Read,
    asset: &Asset,
    destination: &Path,
    cancel: &CancelToken,
    mut progress: impl FnMut(u64),
) -> Result<PathBuf> {
    check_cancel(cancel)?;
    if asset.size == 0 || asset.size > MAX_DOWNLOAD {
        return Err(Failure::new("The update size is invalid."));
    }
    let expected = hex::<32>(&asset.sha256)?;
    let parent = destination
        .parent()
        .ok_or_else(|| Failure::new("Choose a local destination folder."))?;
    // Never overwrite a user file, even after the chooser's overwrite prompt.
    if destination.symlink_metadata().is_ok() {
        return Err(Failure::new(
            "A file already exists at this location. Choose a new filename.",
        ));
    }
    let mut file = tempfile::Builder::new()
        .prefix(".commander-download-")
        .tempfile_in(parent)?;
    let mut digest = Sha256::new();
    let mut received = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        check_cancel(cancel)?;
        received += count as u64;
        if received > asset.size {
            return Err(Failure::new(
                "The download is larger than the signed package size.",
            ));
        }
        digest.update(&buffer[..count]);
        file.write_all(&buffer[..count])?;
        progress(received);
    }
    check_cancel(cancel)?;
    if received != asset.size || digest.finalize().as_slice() != expected {
        return Err(Failure::new(
            "Download verification failed. The incomplete or changed file was removed.",
        ));
    }
    #[cfg(unix)]
    if asset.format == Format::AppImage {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    file.as_file().sync_all()?;
    check_cancel(cancel)?;
    // persist_noclobber is atomic with respect to other files appearing here.
    file.persist_noclobber(destination)
        .map_err(|error| Failure::from(error.error))?;
    Ok(destination.to_owned())
}
