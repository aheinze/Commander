use std::io::{BufReader, Read};

use dualpane_core::{CancelToken, EntryKind, VPath};
use dualpane_vfs::Vfs;
use sha2::{Digest, Sha256};

mod compare;
pub mod duplicates;
pub mod file_compare;
pub mod rename_preview;
pub use compare::{
    CompareEntry, SyncAction, SyncActionKind, SyncDirection, SyncPlan, compare_directories,
    compare_with_contents, execute_sync_plan, plan_sync,
};

mod search;
pub use search::{MAX_CONTENT_BYTES, SearchHit, SearchOptions, SearchResults, recursive_search};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageOutputFormat {
    Png,
    Jpeg,
    WebP,
    Bmp,
}

impl ImageOutputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
            Self::Bmp => "bmp",
        }
    }

    const fn image_format(self) -> image::ImageFormat {
        match self {
            Self::Png => image::ImageFormat::Png,
            Self::Jpeg => image::ImageFormat::Jpeg,
            Self::WebP => image::ImageFormat::WebP,
            Self::Bmp => image::ImageFormat::Bmp,
        }
    }
}

/// Cheap menu/command eligibility using the converter's enabled decoders.
/// Actual image contents are validated by the decoder when conversion starts.
pub fn supports_image_conversion(path: &VPath, kind: EntryKind) -> bool {
    kind == EntryKind::File
        && path
            .as_path()
            .extension()
            .and_then(image::ImageFormat::from_extension)
            .is_some_and(|format| format.reading_enabled())
}

pub fn convert_image(
    vfs: &dyn Vfs,
    source: &VPath,
    destination: &VPath,
    format: ImageOutputFormat,
    cancel: &CancelToken,
) -> Result<(), String> {
    cancel
        .check()
        .map_err(|_| "Image conversion cancelled".to_owned())?;
    let reader = BufReader::new(vfs.open_read(source).map_err(|error| error.to_string())?);
    let mut reader = image::ImageReader::new(reader);
    // Formats such as TGA have no identifying header. Keep the extension as a
    // fallback while allowing recognized file contents to override it.
    if let Ok(format) = image::ImageFormat::from_path(source.as_path()) {
        reader.set_format(format);
    }
    let image = reader
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())?;
    cancel
        .check()
        .map_err(|_| "Image conversion cancelled".to_owned())?;
    let mut writer = vfs
        .create_write(destination, dualpane_core::SizeHint::Unknown)
        .map_err(|error| error.to_string())?;
    image
        .write_to(&mut writer, format.image_format())
        .map_err(|error| error.to_string())
}

const READ_BUFFER_SIZE: usize = 128 * 1_024;
pub fn sha256(vfs: &dyn Vfs, path: &VPath, cancel: &CancelToken) -> Result<String, String> {
    let mut reader = vfs.open_read(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; READ_BUFFER_SIZE];
    loop {
        cancel
            .check()
            .map_err(|_| "Checksum cancelled".to_owned())?;
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dualpane_core::{CancelToken, VPath};
    use dualpane_vfs::LocalFs;

    use super::compare::CompareStatus;
    use super::{SearchOptions, compare_directories, recursive_search, sha256};

    #[test]
    fn search_checksum_and_compare_use_the_vfs() {
        let fixture = tempfile::tempdir().expect("fixture");
        let left = fixture.path().join("left");
        let right = fixture.path().join("right");
        fs::create_dir_all(&left).expect("left");
        fs::create_dir_all(&right).expect("right");
        fs::write(left.join("needle.txt"), "find the needle").expect("left file");
        fs::write(right.join("needle.txt"), "different").expect("right file");
        let cancel = CancelToken::new();

        let hits = recursive_search(
            &LocalFs,
            &VPath::from(left.as_path()),
            &SearchOptions {
                query: "needle".to_owned(),
                search_content: true,
                ..SearchOptions::default()
            },
            &cancel,
        )
        .expect("search");
        assert_eq!(hits.hits.len(), 1);
        assert_eq!(
            sha256(&LocalFs, &VPath::from(left.join("needle.txt")), &cancel).expect("checksum"),
            "9d6a242e1a2b40d9b3d72fbf34f34aac8a26e894868c78be4cd86610a72d7068"
        );
        let compared = compare_directories(
            &LocalFs,
            &VPath::from(left.as_path()),
            &VPath::from(right.as_path()),
            &cancel,
        )
        .expect("compare");
        assert_eq!(compared[0].status, CompareStatus::Different);
    }
}
