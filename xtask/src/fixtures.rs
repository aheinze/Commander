use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dualpane_core::CancelToken;

const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureOptions {
    pub root: PathBuf,
    pub files: u64,
    pub files_per_directory: u64,
    pub payload_bytes: usize,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            root: PathBuf::from("target/fixtures/large-tree"),
            files: 100_000,
            files_per_directory: 100,
            payload_bytes: 32,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureReport {
    pub root: PathBuf,
    pub files: u64,
    pub directories: u64,
    pub bytes_written: u64,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub enum FixtureError {
    Cancelled,
    DestinationExists(PathBuf),
    InvalidOptions(String),
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for FixtureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("fixture generation cancelled"),
            Self::DestinationExists(path) => write!(
                formatter,
                "fixture destination already exists: {}; choose an empty path",
                path.display()
            ),
            Self::InvalidOptions(message) => formatter.write_str(message),
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "filesystem error at {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for FixtureError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Cancelled | Self::DestinationExists(_) | Self::InvalidOptions(_) => None,
        }
    }
}

pub fn generate(
    options: &FixtureOptions,
    cancel: &CancelToken,
) -> Result<FixtureReport, FixtureError> {
    validate(options)?;
    check_cancelled(cancel)?;

    if options.root.exists() {
        return Err(FixtureError::DestinationExists(options.root.clone()));
    }

    let parent = options.root.parent().unwrap_or_else(|| Path::new("."));
    create_directory(parent)?;

    let staging_path = staging_path(&options.root)?;
    create_directory(&staging_path)?;
    let mut staging = StagingDirectory::new(staging_path);
    let started = Instant::now();
    let mut payload = vec![0_u8; options.payload_bytes];
    let mut current_bucket = u64::MAX;
    let mut bucket_path = PathBuf::new();

    for file_index in 0..options.files {
        check_cancelled(cancel)?;
        let bucket = file_index / options.files_per_directory;
        if bucket != current_bucket {
            bucket_path = staging.path().join(format!("bucket-{bucket:06}"));
            create_directory(&bucket_path)?;
            current_bucket = bucket;
        }

        prepare_payload(&mut payload, file_index);
        let file_path = bucket_path.join(format!("entry-{file_index:08}.bin"));
        write_file(&file_path, &payload)?;
    }

    check_cancelled(cancel)?;
    rename_path(staging.path(), &options.root)?;
    staging.disarm();

    let directories = options.files.div_ceil(options.files_per_directory);
    let bytes_written = options
        .files
        .saturating_mul(u64::try_from(options.payload_bytes).unwrap_or(u64::MAX));

    Ok(FixtureReport {
        root: options.root.clone(),
        files: options.files,
        directories,
        bytes_written,
        elapsed: started.elapsed(),
    })
}

fn validate(options: &FixtureOptions) -> Result<(), FixtureError> {
    if options.files == 0 {
        return Err(FixtureError::InvalidOptions(
            "fixture file count must be greater than zero".to_owned(),
        ));
    }
    if options.files_per_directory == 0 {
        return Err(FixtureError::InvalidOptions(
            "files per directory must be greater than zero".to_owned(),
        ));
    }
    if options.payload_bytes > MAX_PAYLOAD_BYTES {
        return Err(FixtureError::InvalidOptions(format!(
            "payload size may not exceed {MAX_PAYLOAD_BYTES} bytes"
        )));
    }
    if options.root.file_name().is_none() {
        return Err(FixtureError::InvalidOptions(
            "fixture destination must name a directory".to_owned(),
        ));
    }
    Ok(())
}

fn check_cancelled(cancel: &CancelToken) -> Result<(), FixtureError> {
    cancel.check().map_err(|_| FixtureError::Cancelled)
}

fn staging_path(root: &Path) -> Result<PathBuf, FixtureError> {
    let file_name = root.file_name().ok_or_else(|| {
        FixtureError::InvalidOptions("fixture destination must name a directory".to_owned())
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging_name = format!(
        ".{}.dualpane-staging-{}-{nonce}",
        file_name.to_string_lossy(),
        process::id()
    );
    Ok(root.with_file_name(staging_name))
}

fn prepare_payload(payload: &mut [u8], file_index: u64) {
    for (offset, byte) in payload.iter_mut().enumerate() {
        let shift = (offset % 8) * 8;
        *byte = ((file_index >> shift) as u8).wrapping_add(offset as u8);
    }
}

fn create_directory(path: &Path) -> Result<(), FixtureError> {
    fs::create_dir_all(path).map_err(|source| FixtureError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), FixtureError> {
    let mut file = File::create(path).map_err(|source| FixtureError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(contents).map_err(|source| FixtureError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn rename_path(from: &Path, to: &Path) -> Result<(), FixtureError> {
    fs::rename(from, to).map_err(|source| FixtureError::Io {
        path: to.to_path_buf(),
        source,
    })
}

struct StagingDirectory {
    path: PathBuf,
    armed: bool,
}

impl StagingDirectory {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use dualpane_core::CancelToken;

    use super::{FixtureError, FixtureOptions, generate};

    #[test]
    fn generates_the_requested_deterministic_shape() {
        let root = temporary_root("shape");
        let options = FixtureOptions {
            root: root.clone(),
            files: 17,
            files_per_directory: 5,
            payload_bytes: 8,
        };

        let report = generate(&options, &CancelToken::new()).expect("generate fixture");

        assert_eq!(report.files, 17);
        assert_eq!(report.directories, 4);
        assert_eq!(report.bytes_written, 136);
        assert_eq!(count_files(&root), 17);
        assert_eq!(
            fs::read(root.join("bucket-000000/entry-00000000.bin"))
                .unwrap()
                .len(),
            8
        );
        assert_eq!(
            fs::read(root.join("bucket-000003/entry-00000016.bin"))
                .unwrap()
                .len(),
            8
        );

        fs::remove_dir_all(root).expect("remove test fixture");
    }

    #[test]
    fn cancelled_generation_does_not_create_the_destination() {
        let root = temporary_root("cancelled");
        let options = FixtureOptions {
            root: root.clone(),
            files: 3,
            files_per_directory: 1,
            payload_bytes: 0,
        };
        let cancel = CancelToken::new();
        cancel.cancel();

        let error = generate(&options, &cancel).expect_err("generation must cancel");

        assert!(matches!(error, FixtureError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn refuses_to_overwrite_an_existing_destination() {
        let root = temporary_root("exists");
        fs::create_dir_all(&root).expect("create destination");
        let options = FixtureOptions {
            root: root.clone(),
            files: 1,
            files_per_directory: 1,
            payload_bytes: 0,
        };

        let error = generate(&options, &CancelToken::new()).expect_err("must refuse overwrite");

        assert!(matches!(error, FixtureError::DestinationExists(path) if path == root));
        fs::remove_dir_all(root).expect("remove test destination");
    }

    fn temporary_root(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "dualpane-xtask-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn count_files(root: &std::path::Path) -> usize {
        fs::read_dir(root)
            .expect("read fixture root")
            .map(|entry| entry.expect("read bucket"))
            .map(|entry| {
                fs::read_dir(entry.path())
                    .expect("read bucket contents")
                    .count()
            })
            .sum()
    }
}
