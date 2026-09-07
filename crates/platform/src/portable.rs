use std::fs::{File, OpenOptions, ReadDir};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use dualpane_core::{
    CancelToken, Capabilities, CapabilitySupport, Entry, EntryKind, Metadata, Timestamp,
};

use crate::NativeTrashLocation;

pub fn set_mode_elevated(
    _path: &Path,
    _mode: u32,
    _recursive: bool,
    _cancel: &CancelToken,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "elevated permission changes are unavailable on this platform",
    ))
}

/// Portable fallback directory stream used by future non-Unix builds.
pub struct NativeDirectory {
    directory: ReadDir,
}

impl Iterator for NativeDirectory {
    type Item = io::Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.directory.next()?;
        Some(entry.and_then(|entry| {
            let kind = entry.file_type().map(entry_kind)?;
            Ok(Entry::new(entry.file_name(), kind, None))
        }))
    }
}

pub fn read_directory(path: &Path) -> io::Result<NativeDirectory> {
    std::fs::read_dir(path).map(|directory| NativeDirectory { directory })
}

pub fn metadata(path: &Path, follow: bool) -> io::Result<Metadata> {
    let native = if follow {
        std::fs::metadata(path)?
    } else {
        std::fs::symlink_metadata(path)?
    };
    Ok(Metadata {
        kind: entry_kind(native.file_type()),
        identity: None,
        size: native.len(),
        allocated_size: native.len(),
        modified: native.modified().ok().map(timestamp),
        created: native.created().ok().map(timestamp),
        accessed: native.accessed().ok().map(timestamp),
        mode: None,
        owner: None,
        group: None,
        hard_links: None,
    })
}

pub fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Unix permission modes are unavailable on this platform",
    ))
}

pub fn open_read(path: &Path) -> io::Result<File> {
    File::open(path)
}

pub fn create_write(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

pub fn create_reflink(_source: &Path, destination: &Path) -> io::Result<bool> {
    drop(create_write(destination)?);
    Ok(false)
}

pub fn open_write(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).open(path)
}

pub fn create_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

pub fn read_link(path: &Path) -> io::Result<PathBuf> {
    std::fs::read_link(path)
}

#[cfg(windows)]
pub fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    if target.is_dir() {
        symlink_dir(target, link)
    } else {
        symlink_file(target, link)
    }
}

#[cfg(not(windows))]
pub fn create_symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symbolic links are unsupported on this platform",
    ))
}

pub fn hard_link(source: &Path, link: &Path) -> io::Result<()> {
    std::fs::hard_link(source, link)
}

pub fn reflink(_source: &Path, _destination: &Path) -> io::Result<bool> {
    Ok(false)
}

pub fn copy_file_range(
    _source: &Path,
    _destination: &Path,
    _offset: u64,
    _length: u64,
    cancel: &dualpane_core::CancelToken,
    _progress: &mut dyn FnMut(u64),
) -> io::Result<u64> {
    cancel
        .check()
        .map_err(|_| io::Error::from(io::ErrorKind::Interrupted))?;
    Ok(0)
}

pub fn sparse_ranges(_path: &Path, _length: u64) -> io::Result<Option<Vec<(u64, u64)>>> {
    Ok(None)
}

pub fn set_len(path: &Path, length: u64) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.set_len(length)
}

pub fn allocate_range(_path: &Path, _offset: u64, _length: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "range allocation is unsupported on this platform",
    ))
}

pub fn preserve_metadata(
    _source: &Path,
    destination: &Path,
    metadata: &Metadata,
) -> io::Result<Vec<String>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(destination)?;
    let mut times = std::fs::FileTimes::new();
    if let Some(accessed) = metadata.accessed {
        times = times.set_accessed(system_time(accessed));
    }
    if let Some(modified) = metadata.modified {
        times = times.set_modified(system_time(modified));
    }
    file.set_times(times)?;
    Ok(Vec::new())
}

pub fn sync_file(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

pub fn available_space(_path: &Path) -> io::Result<Option<u64>> {
    Ok(None)
}

pub fn prepare_trash(_source: &Path) -> io::Result<NativeTrashLocation> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory is unavailable"))?;
    let root = data_home.join("Trash");
    let files = root.join("files");
    let info = root.join("info");
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    Ok(NativeTrashLocation {
        files,
        info,
        deletion_date: "1970-01-01T00:00:00".to_owned(),
    })
}

#[must_use]
pub fn recommended_copy_concurrency(_path: &Path) -> usize {
    std::thread::available_parallelism().map_or(1, |count| count.get().min(4))
}

pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub fn remove(path: &Path, kind: EntryKind) -> io::Result<()> {
    if kind.is_directory() {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[must_use]
pub const fn capabilities() -> Capabilities {
    Capabilities {
        stable_identity: CapabilitySupport::Unsupported,
        atomic_rename: CapabilitySupport::FilesystemDependent,
        hard_links: CapabilitySupport::FilesystemDependent,
        reflink: CapabilitySupport::Unsupported,
        copy_file_range: CapabilitySupport::Unsupported,
        sparse_files: CapabilitySupport::FilesystemDependent,
        extended_attributes: CapabilitySupport::FilesystemDependent,
        file_watching: CapabilitySupport::Supported,
    }
}

#[must_use]
pub const fn process_rss_kib() -> Option<u64> {
    None
}

fn entry_kind(file_type: std::fs::FileType) -> EntryKind {
    if file_type.is_file() {
        EntryKind::File
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::Unknown
    }
}

fn timestamp(value: SystemTime) -> Timestamp {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => Timestamp {
            seconds: i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
            nanoseconds: duration.subsec_nanos(),
        },
        Err(error) => {
            let duration = error.duration();
            Timestamp {
                seconds: -i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
                nanoseconds: duration.subsec_nanos(),
            }
        }
    }
}

fn system_time(value: Timestamp) -> SystemTime {
    let duration = std::time::Duration::new(value.seconds.unsigned_abs(), value.nanoseconds);
    if value.seconds < 0 {
        UNIX_EPOCH.checked_sub(duration).unwrap_or(UNIX_EPOCH)
    } else {
        UNIX_EPOCH.checked_add(duration).unwrap_or(UNIX_EPOCH)
    }
}

/// Windows rename refuses existing destinations. Other platforms fail closed.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        std::fs::rename(from, to)
    }
    #[cfg(not(windows))]
    {
        let _ = (from, to);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-replace rename is unavailable",
        ))
    }
}

/// Resolves aliases for transfer ancestry checks.
pub fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}
