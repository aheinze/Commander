//! Credentials stay in memory and never enter debug output, sessions or journals.
use super::*;
use std::{fmt, sync::Arc};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Password(Arc<Zeroizing<String>>);

impl Password {
    pub fn new(value: String) -> Option<Self> {
        let value = Zeroizing::new(value);
        (!value.is_empty()).then(|| Self(Arc::new(value)))
    }

    pub(super) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password([redacted])")
    }
}

pub(super) fn sevenz_password(password: Option<&Password>) -> SevenZPassword {
    password.map_or_else(SevenZPassword::empty, |password| {
        SevenZPassword::new(password.expose())
    })
}

pub(super) fn configure_sevenz<W: Write + io::Seek>(
    writer: &mut SevenZWriter<W>,
    password: Option<&Password>,
) {
    if let Some(password) = password {
        use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
        writer.set_content_methods(vec![
            AesEncoderOptions::new(sevenz_password(Some(password))).into(),
            Lzma2Options::default().into(),
        ]);
        writer.set_encrypt_header(true);
    }
}

pub(super) fn zip_options(password: Option<&Password>) -> FileOptions<'_> {
    let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
    password.map_or(options, |password| {
        options.with_aes_encryption(zip::AesMode::Aes256, password.expose())
    })
}

fn encrypted(vfs: &dyn Vfs, source: &VPath, task: &ArchiveTask<'_>) -> Result<bool, String> {
    let name = source.to_string().to_ascii_lowercase();
    if name.ends_with(".zip") {
        let mut archive = ZipArchive::new(vfs.open_read(source).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for index in 0..archive.len() {
            task.check().map_err(|_| "Archive operation cancelled")?;
            if archive
                .by_index_raw(index)
                .map_err(|e| e.to_string())?
                .encrypted()
            {
                return Ok(true);
            }
        }
    } else if name.ends_with(".7z") {
        match SevenZReader::new(
            vfs.open_read(source).map_err(|e| e.to_string())?,
            SevenZPassword::empty(),
        ) {
            Ok(archive) => {
                return Ok(archive.archive().blocks.iter().any(|block| {
                    block.coders.iter().any(|coder| {
                        coder.encoder_method_id() == sevenz_rust2::EncoderMethod::ID_AES256_SHA256
                    })
                }));
            }
            Err(sevenz_rust2::Error::PasswordRequired) => return Ok(true),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(false)
}

// Authenticate/decompress every stream before touching an extraction destination.
// ZipCrypto's short password check can otherwise accept an incorrect password.
// 7z has no independent password verifier, so decoding is required there too.
fn validate(
    vfs: &dyn Vfs,
    source: &VPath,
    password: &Password,
    task: &ArchiveTask<'_>,
) -> Result<(), ValidationError> {
    let reader = vfs
        .open_read(source)
        .map_err(|e| ValidationError::Other(e.to_string()))?;
    if source.to_string().to_ascii_lowercase().ends_with(".zip") {
        let mut archive = ZipArchive::new(reader).map_err(ValidationError::zip)?;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index_decrypt(index, password.expose().as_bytes())
                .map_err(ValidationError::zip)?;
            drain(&mut entry, task).map_err(ValidationError::io)?;
        }
    } else {
        let mut archive = SevenZReader::new(reader, sevenz_password(Some(password)))
            .map_err(ValidationError::sevenz)?;
        archive
            .for_each_entries(|_, reader| {
                drain(reader, task).map_err(sevenz_rust2::Error::from)?;
                Ok(true)
            })
            .map_err(ValidationError::sevenz)?;
    }
    Ok(())
}

fn drain(reader: &mut dyn Read, task: &ArchiveTask<'_>) -> io::Result<()> {
    let mut buffer = [0; BUFFER_SIZE];
    loop {
        task.check().map_err(|_| {
            io::Error::new(io::ErrorKind::Interrupted, "Archive operation cancelled")
        })?;
        if reader.read(&mut buffer)? == 0 {
            return Ok(());
        }
    }
}

enum ValidationError {
    Password,
    Other(String),
}
impl ValidationError {
    fn zip(error: zip::result::ZipError) -> Self {
        match error {
            zip::result::ZipError::InvalidPassword => Self::Password,
            zip::result::ZipError::Io(error) => Self::io(error),
            error => Self::Other(error.to_string()),
        }
    }
    fn io(error: io::Error) -> Self {
        if matches!(
            error.kind(),
            io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof | io::ErrorKind::Other
        ) {
            Self::Password
        } else {
            Self::Other(error.to_string())
        }
    }
    fn sevenz(error: sevenz_rust2::Error) -> Self {
        match error {
            sevenz_rust2::Error::PasswordRequired
            | sevenz_rust2::Error::MaybeBadPassword(_)
            | sevenz_rust2::Error::ChecksumVerificationFailed => Self::Password,
            sevenz_rust2::Error::Io(error, _) => Self::io(error),
            error => Self::Other(error.to_string()),
        }
    }
}

pub(super) fn unlock(
    vfs: &dyn Vfs,
    source: &VPath,
    task: &mut ArchiveTask<'_>,
) -> Result<(), String> {
    if !encrypted(vfs, source, task)? {
        task.password = None;
        return Ok(());
    }
    let mut incorrect = false;
    loop {
        task.check().map_err(|_| "Archive operation cancelled")?;
        if let Some(password) = &task.password {
            match validate(vfs, source, password, task) {
                Ok(()) => return Ok(()),
                Err(ValidationError::Password) => {
                    incorrect = true;
                    task.password = None;
                }
                Err(ValidationError::Other(error)) => return Err(error),
            }
        }
        task.check().map_err(|_| "Archive operation cancelled")?;
        let Some(prompt) = task.password_prompt.as_mut() else {
            return Err(if incorrect {
                "The password is incorrect, or the archive is damaged"
            } else {
                "This archive requires a password"
            }
            .into());
        };
        task.password = prompt(source, incorrect);
        if task.password.is_none() {
            task.cancel.cancel();
            return Err("Opening encrypted archive cancelled".into());
        }
    }
}

#[cfg(test)]
mod tests;
