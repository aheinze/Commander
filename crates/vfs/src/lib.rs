#![forbid(unsafe_code)]

//! Virtual filesystem abstractions and the local filesystem backend.

use std::io::{Read, Seek, Write};
use std::path::PathBuf;

use dualpane_core::{CancelToken, Capabilities, Entry, EntryKind, Metadata, SizeHint, VPath};
use thiserror::Error;

/// A seekable input stream returned by a VFS implementation.
pub trait ReadSeek: Read + Seek + Send {}

impl<T: Read + Seek + Send> ReadSeek for T {}

/// A seekable output stream returned by a VFS implementation.
pub trait WriteSeek: Write + Seek + Send {}

impl<T: Write + Seek + Send> WriteSeek for T {}

/// Errors returned through the UI-independent VFS seam.
#[derive(Debug, Error)]
pub enum VfsError {
    #[error("operation cancelled")]
    Cancelled,
    #[error("{operation} is unsupported for {path}")]
    Unsupported {
        operation: &'static str,
        path: VPath,
    },
    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: VPath,
        #[source]
        source: std::io::Error,
    },
}

impl VfsError {
    /// Returns the underlying I/O category when this is a filesystem error.
    #[must_use]
    pub fn io_kind(&self) -> Option<std::io::ErrorKind> {
        match self {
            Self::Cancelled | Self::Unsupported { .. } => None,
            Self::Io { source, .. } => Some(source.kind()),
        }
    }

    /// Returns the native OS error number for platform-sensitive fallback decisions.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        match self {
            Self::Io { source, .. } => source.raw_os_error(),
            Self::Cancelled | Self::Unsupported { .. } => None,
        }
    }
}

/// Result type used by VFS operations.
pub type Result<T> = std::result::Result<T, VfsError>;

/// XDG trash directories prepared on the source filesystem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrashLocation {
    pub files: VPath,
    pub info: VPath,
    pub deletion_date: String,
}

/// UI-independent filesystem seam used by the index and operation engine.
///
/// Methods are synchronous by design. Orchestration crates must call them exclusively
/// on worker threads, which makes the "UI thread never touches the filesystem" rule
/// enforceable at the application boundary.
pub trait Vfs: Send + Sync + 'static {
    fn read_dir(
        &self,
        path: &VPath,
        cancel: &CancelToken,
    ) -> Result<Box<dyn Iterator<Item = Result<Entry>> + Send>>;

    fn stat(&self, path: &VPath, follow: bool) -> Result<Metadata>;

    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadSeek>>;

    fn create_write(&self, path: &VPath, hint: SizeHint) -> Result<Box<dyn WriteSeek>>;

    /// Creates a new destination and attempts to clone `source` into the same open file.
    ///
    /// The destination exists on both `Ok(true)` and `Ok(false)`, allowing callers to
    /// continue with another copy strategy without reopening it for creation.
    /// On error, remove only a destination created by this call; never remove
    /// a pre-existing object when exclusive creation fails.
    fn create_reflink(&self, source: &VPath, destination: &VPath, hint: SizeHint) -> Result<bool> {
        drop(self.create_write(destination, hint)?);
        match self.reflink(source, destination) {
            Ok(cloned) => Ok(cloned),
            Err(VfsError::Unsupported { .. }) => Ok(false),
            Err(error) => {
                let _ = self.remove(destination, EntryKind::File);
                Err(error)
            }
        }
    }

    fn open_write(&self, path: &VPath) -> Result<Box<dyn WriteSeek>> {
        Err(unsupported("open for writing", path))
    }

    fn create_dir(&self, path: &VPath) -> Result<()> {
        Err(unsupported("create directory", path))
    }

    fn read_link(&self, path: &VPath) -> Result<PathBuf> {
        Err(unsupported("read symbolic link", path))
    }

    fn create_symlink(&self, target: &VPath, link: &VPath) -> Result<()> {
        let _ = target;
        Err(unsupported("create symbolic link", link))
    }

    fn hard_link(&self, source: &VPath, link: &VPath) -> Result<()> {
        let _ = source;
        Err(unsupported("create hard link", link))
    }

    /// Attempts a copy-on-write clone and returns whether it succeeded.
    fn reflink(&self, source: &VPath, destination: &VPath) -> Result<bool> {
        let _ = source;
        Err(unsupported("reflink", destination))
    }

    /// Copies as much as possible with a native range-copy primitive.
    fn copy_file_range(
        &self,
        source: &VPath,
        destination: &VPath,
        offset: u64,
        length: u64,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(u64),
    ) -> Result<u64> {
        let _ = (source, offset, length, cancel, progress);
        Err(unsupported("copy file range", destination))
    }

    /// Returns data extents as half-open byte ranges, or `None` when unsupported.
    fn sparse_ranges(&self, path: &VPath, length: u64) -> Result<Option<Vec<(u64, u64)>>> {
        let _ = length;
        Err(unsupported("query sparse extents", path))
    }

    fn set_len(&self, path: &VPath, length: u64) -> Result<()> {
        let _ = length;
        Err(unsupported("set file length", path))
    }

    fn set_mode(&self, path: &VPath, mode: u32) -> Result<()> {
        let _ = mode;
        Err(unsupported("change permissions", path))
    }

    fn allocate_range(&self, path: &VPath, offset: u64, length: u64) -> Result<()> {
        let _ = (offset, length);
        Err(unsupported("allocate file range", path))
    }

    /// Preserves attributes best-effort and returns non-fatal warning messages.
    fn preserve_metadata(
        &self,
        source: &VPath,
        destination: &VPath,
        metadata: &Metadata,
    ) -> Result<Vec<String>> {
        let _ = (source, metadata);
        Err(unsupported("preserve metadata", destination))
    }

    fn sync_file(&self, path: &VPath) -> Result<()> {
        Err(unsupported("synchronize file", path))
    }

    fn available_space(&self, path: &VPath) -> Result<Option<u64>> {
        Err(unsupported("query available space", path))
    }

    fn prepare_trash(&self, source: &VPath) -> Result<TrashLocation> {
        Err(unsupported("prepare trash", source))
    }

    fn recommended_copy_concurrency(&self, path: &VPath) -> usize {
        let _ = path;
        1
    }

    fn canonicalize(&self, path: &VPath) -> Result<VPath> {
        Err(unsupported("resolve canonical path", path))
    }

    fn rename(&self, from: &VPath, to: &VPath) -> Result<()>;

    /// Must atomically refuse to replace any existing destination.
    fn rename_noreplace(&self, from: &VPath, to: &VPath) -> Result<()> {
        let _ = from;
        Err(unsupported("rename without replacement", to))
    }

    fn remove(&self, path: &VPath, kind: EntryKind) -> Result<()>;

    fn capabilities(&self) -> Capabilities;
}

/// Native local-filesystem implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalFs;

impl Vfs for LocalFs {
    fn read_dir(
        &self,
        path: &VPath,
        cancel: &CancelToken,
    ) -> Result<Box<dyn Iterator<Item = Result<Entry>> + Send>> {
        cancel.check().map_err(|_| VfsError::Cancelled)?;
        let native = dualpane_platform::read_directory(path.as_path())
            .map_err(|source| io_error("read directory", path, source))?;

        Ok(Box::new(LocalDirectoryIterator {
            native,
            cancel: cancel.clone(),
            path: path.clone(),
            finished: false,
        }))
    }

    fn stat(&self, path: &VPath, follow: bool) -> Result<Metadata> {
        dualpane_platform::metadata(path.as_path(), follow)
            .map_err(|source| io_error("stat", path, source))
    }

    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadSeek>> {
        dualpane_platform::open_read(path.as_path())
            .map(|file| Box::new(file) as Box<dyn ReadSeek>)
            .map_err(|source| io_error("open for reading", path, source))
    }

    fn create_write(&self, path: &VPath, _hint: SizeHint) -> Result<Box<dyn WriteSeek>> {
        dualpane_platform::create_write(path.as_path())
            .map(|file| Box::new(file) as Box<dyn WriteSeek>)
            .map_err(|source| io_error("create for writing", path, source))
    }

    fn create_reflink(&self, source: &VPath, destination: &VPath, _hint: SizeHint) -> Result<bool> {
        dualpane_platform::create_reflink(source.as_path(), destination.as_path())
            .map_err(|source_error| io_error("create reflink", destination, source_error))
    }

    fn open_write(&self, path: &VPath) -> Result<Box<dyn WriteSeek>> {
        dualpane_platform::open_write(path.as_path())
            .map(|file| Box::new(file) as Box<dyn WriteSeek>)
            .map_err(|source| io_error("open for writing", path, source))
    }

    fn create_dir(&self, path: &VPath) -> Result<()> {
        dualpane_platform::create_dir(path.as_path())
            .map_err(|source| io_error("create directory", path, source))
    }

    fn read_link(&self, path: &VPath) -> Result<PathBuf> {
        dualpane_platform::read_link(path.as_path())
            .map_err(|source| io_error("read symbolic link", path, source))
    }

    fn create_symlink(&self, target: &VPath, link: &VPath) -> Result<()> {
        dualpane_platform::create_symlink(target.as_path(), link.as_path())
            .map_err(|source| io_error("create symbolic link", link, source))
    }

    fn hard_link(&self, source: &VPath, link: &VPath) -> Result<()> {
        dualpane_platform::hard_link(source.as_path(), link.as_path())
            .map_err(|source_error| io_error("create hard link", link, source_error))
    }

    fn reflink(&self, source: &VPath, destination: &VPath) -> Result<bool> {
        dualpane_platform::reflink(source.as_path(), destination.as_path())
            .map_err(|source_error| io_error("reflink", destination, source_error))
    }

    fn copy_file_range(
        &self,
        source: &VPath,
        destination: &VPath,
        offset: u64,
        length: u64,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(u64),
    ) -> Result<u64> {
        dualpane_platform::copy_file_range(
            source.as_path(),
            destination.as_path(),
            offset,
            length,
            cancel,
            progress,
        )
        .map_err(|source_error| {
            if cancel.is_cancelled() {
                VfsError::Cancelled
            } else {
                io_error("copy file range", destination, source_error)
            }
        })
    }

    fn sparse_ranges(&self, path: &VPath, length: u64) -> Result<Option<Vec<(u64, u64)>>> {
        dualpane_platform::sparse_ranges(path.as_path(), length)
            .map_err(|source| io_error("query sparse extents", path, source))
    }

    fn set_len(&self, path: &VPath, length: u64) -> Result<()> {
        dualpane_platform::set_len(path.as_path(), length)
            .map_err(|source| io_error("set file length", path, source))
    }

    fn set_mode(&self, path: &VPath, mode: u32) -> Result<()> {
        dualpane_platform::set_mode(path.as_path(), mode)
            .map_err(|source| io_error("change permissions", path, source))
    }

    fn allocate_range(&self, path: &VPath, offset: u64, length: u64) -> Result<()> {
        dualpane_platform::allocate_range(path.as_path(), offset, length)
            .map_err(|source| io_error("allocate file range", path, source))
    }

    fn preserve_metadata(
        &self,
        source: &VPath,
        destination: &VPath,
        metadata: &Metadata,
    ) -> Result<Vec<String>> {
        dualpane_platform::preserve_metadata(source.as_path(), destination.as_path(), metadata)
            .map_err(|source_error| io_error("preserve metadata", destination, source_error))
    }

    fn sync_file(&self, path: &VPath) -> Result<()> {
        dualpane_platform::sync_file(path.as_path())
            .map_err(|source| io_error("synchronize file", path, source))
    }

    fn available_space(&self, path: &VPath) -> Result<Option<u64>> {
        dualpane_platform::available_space(path.as_path())
            .map_err(|source| io_error("query available space", path, source))
    }

    fn prepare_trash(&self, source: &VPath) -> Result<TrashLocation> {
        dualpane_platform::prepare_trash(source.as_path())
            .map(|location| TrashLocation {
                files: VPath::from(location.files),
                info: VPath::from(location.info),
                deletion_date: location.deletion_date,
            })
            .map_err(|source_error| io_error("prepare trash", source, source_error))
    }

    fn recommended_copy_concurrency(&self, path: &VPath) -> usize {
        dualpane_platform::recommended_copy_concurrency(path.as_path())
    }

    fn canonicalize(&self, path: &VPath) -> Result<VPath> {
        dualpane_platform::canonicalize(path.as_path())
            .map(VPath::from)
            .map_err(|source| io_error("resolve canonical path", path, source))
    }

    fn rename_noreplace(&self, from: &VPath, to: &VPath) -> Result<()> {
        dualpane_platform::rename_noreplace(from.as_path(), to.as_path())
            .map_err(|source| io_error("rename without replacement", to, source))
    }

    fn rename(&self, from: &VPath, to: &VPath) -> Result<()> {
        dualpane_platform::rename(from.as_path(), to.as_path()).map_err(|source| VfsError::Io {
            operation: "rename",
            path: from.clone(),
            source,
        })
    }

    fn remove(&self, path: &VPath, kind: EntryKind) -> Result<()> {
        dualpane_platform::remove(path.as_path(), kind)
            .map_err(|source| io_error("remove", path, source))
    }

    fn capabilities(&self) -> Capabilities {
        dualpane_platform::capabilities()
    }
}

struct LocalDirectoryIterator {
    native: dualpane_platform::NativeDirectory,
    cancel: CancelToken,
    path: VPath,
    finished: bool,
}

impl Iterator for LocalDirectoryIterator {
    type Item = Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        if self.cancel.is_cancelled() {
            self.finished = true;
            return Some(Err(VfsError::Cancelled));
        }

        self.native.next().map(|result| {
            result.map_err(|source| VfsError::Io {
                operation: "read directory entry",
                path: self.path.clone(),
                source,
            })
        })
    }
}

fn io_error(operation: &'static str, path: &VPath, source: std::io::Error) -> VfsError {
    VfsError::Io {
        operation,
        path: path.clone(),
        source,
    }
}

fn unsupported(operation: &'static str, path: &VPath) -> VfsError {
    VfsError::Unsupported {
        operation,
        path: path.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dualpane_core::{CancelToken, EntryKind, VPath};
    use tempfile::tempdir;

    use super::{LocalFs, Vfs, VfsError};

    #[test]
    fn local_fs_lists_dirent_types_without_dot_entries() {
        let fixture = tempdir().expect("fixture");
        fs::write(fixture.path().join("file.txt"), b"hello").expect("file");
        fs::create_dir(fixture.path().join("folder")).expect("folder");
        let root = VPath::from(fixture.path());

        let mut entries: Vec<_> = LocalFs
            .read_dir(&root, &CancelToken::new())
            .expect("read directory")
            .collect::<super::Result<Vec<_>>>()
            .expect("entries");
        entries.sort_by(|left, right| left.name().cmp(right.name()));

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name(), "file.txt");
        assert_eq!(entries[0].kind(), EntryKind::File);
        assert_eq!(entries[1].name(), "folder");
        assert_eq!(entries[1].kind(), EntryKind::Directory);
        assert!(entries.iter().all(|entry| entry.identity().is_some()));
    }

    #[test]
    fn local_fs_returns_nanosecond_metadata() {
        let fixture = tempdir().expect("fixture");
        let file = fixture.path().join("metadata.bin");
        fs::write(&file, b"payload").expect("file");

        let metadata = LocalFs
            .stat(&VPath::from(file.as_path()), false)
            .expect("metadata");

        assert_eq!(metadata.kind, EntryKind::File);
        assert_eq!(metadata.size, 7);
        assert!(metadata.identity.is_some());
        assert!(metadata.modified.is_some());
    }

    #[test]
    fn cancelled_listing_stops_before_opening_the_directory() {
        let cancel = CancelToken::new();
        cancel.cancel();

        let result = LocalFs.read_dir(&VPath::from("does-not-exist"), &cancel);

        assert!(matches!(result, Err(VfsError::Cancelled)));
    }
}
