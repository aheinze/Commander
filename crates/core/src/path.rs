use std::borrow::Borrow;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

/// A lossless native path used by the headless domain and VFS layers.
///
/// UTF-8 conversion belongs at the UI boundary so Unix filenames with arbitrary bytes
/// remain addressable and file operations never target a lossy reconstruction.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct VPath(PathBuf);

impl VPath {
    /// Wraps an owned native path.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// Returns the native path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Returns a path with `name` appended without requiring UTF-8.
    #[must_use]
    pub fn join_name(&self, name: &OsStr) -> Self {
        Self(self.0.join(name))
    }

    /// Returns this path's final component.
    #[must_use]
    pub fn file_name(&self) -> Option<&OsStr> {
        self.0.file_name()
    }

    /// Returns the parent path.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        self.0.parent().map(Path::to_path_buf).map(Self)
    }

    /// Consumes the wrapper and returns its native path buffer.
    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl AsRef<Path> for VPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl Borrow<Path> for VPath {
    fn borrow(&self) -> &Path {
        self.as_path()
    }
}

impl From<PathBuf> for VPath {
    fn from(path: PathBuf) -> Self {
        Self::new(path)
    }
}

impl From<&Path> for VPath {
    fn from(path: &Path) -> Self {
        Self::new(path.to_path_buf())
    }
}

impl From<&str> for VPath {
    fn from(path: &str) -> Self {
        Self::new(PathBuf::from(path))
    }
}

impl fmt::Display for VPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(formatter)
    }
}

// Journal paths round-trip native Unix bytes, including non-UTF-8 filenames.
#[cfg(unix)]
impl serde::Serialize for VPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use std::os::unix::ffi::OsStrExt;
        serializer.serialize_bytes(self.0.as_os_str().as_bytes())
    }
}
#[cfg(unix)]
impl<'de> serde::Deserialize<'de> for VPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use std::os::unix::ffi::OsStringExt;
        let bytes = <Vec<u8> as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self(PathBuf::from(std::ffi::OsString::from_vec(bytes))))
    }
}
