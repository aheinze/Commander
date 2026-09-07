use std::ffi::{OsStr, OsString};

/// Stable native identity for a filesystem object.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

/// Filesystem object type obtained from a directory entry or metadata call.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Fifo,
    Socket,
    CharacterDevice,
    BlockDevice,
    Unknown,
}

impl EntryKind {
    /// Returns whether this entry is a directory.
    #[must_use]
    pub const fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }
}

/// A lightweight directory entry that deliberately excludes full metadata.
///
/// The parent path is interned once by the listing. Keeping only the name here makes
/// the hot 100k-entry representation compact while preserving non-UTF-8 names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    name: Box<OsStr>,
    kind: EntryKind,
    identity: Option<FileIdentity>,
}

impl Entry {
    /// Creates a lightweight directory entry.
    #[must_use]
    pub fn new(name: OsString, kind: EntryKind, identity: Option<FileIdentity>) -> Self {
        Self {
            name: name.into_boxed_os_str(),
            kind,
            identity,
        }
    }

    /// Returns the lossless native filename.
    #[must_use]
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Returns the filesystem object type reported during enumeration.
    #[must_use]
    pub const fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Returns the stable identity when the platform provides one cheaply.
    #[must_use]
    pub const fn identity(&self) -> Option<FileIdentity> {
        self.identity
    }
}

/// A nanosecond-resolution timestamp that supports pre-Unix-epoch values.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanoseconds: u32,
}

/// Full metadata fetched lazily for visible rows or explicit batches.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub kind: EntryKind,
    pub identity: Option<FileIdentity>,
    pub size: u64,
    pub allocated_size: u64,
    pub modified: Option<Timestamp>,
    pub created: Option<Timestamp>,
    pub accessed: Option<Timestamp>,
    pub mode: Option<u32>,
    pub owner: Option<u32>,
    pub group: Option<u32>,
    pub hard_links: Option<u64>,
}

/// Expected output size supplied to a VFS write handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SizeHint {
    Unknown,
    Exact(u64),
}

/// Whether a capability is global or must be resolved for a filesystem/device pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilitySupport {
    Unsupported,
    Supported,
    FilesystemDependent,
}

/// Capabilities advertised by a VFS implementation without platform branching in callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    pub stable_identity: CapabilitySupport,
    pub atomic_rename: CapabilitySupport,
    pub hard_links: CapabilitySupport,
    pub reflink: CapabilitySupport,
    pub copy_file_range: CapabilitySupport,
    pub sparse_files: CapabilitySupport,
    pub extended_attributes: CapabilitySupport,
    pub file_watching: CapabilitySupport,
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::Entry;

    #[test]
    fn lightweight_entry_stays_inside_the_memory_budget() {
        assert!(
            size_of::<Entry>() <= 64,
            "Entry grew to {} bytes",
            size_of::<Entry>()
        );
    }
}
