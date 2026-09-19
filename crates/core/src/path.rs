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

    /// Reversible string for session values and map keys. Ordinary UTF-8 paths
    /// retain their legacy representation. NUL cannot occur in a native filename,
    /// so the encoded form cannot collide with a real legacy path or name.
    #[must_use]
    pub fn to_storage_string(&self) -> String {
        if let Some(text) = self.0.to_str() {
            return text.to_owned();
        }
        let mut encoded = String::from("\0native:");
        for byte in self.0.as_os_str().as_encoded_bytes() {
            use std::fmt::Write;
            let _ = write!(encoded, "{byte:02x}");
        }
        encoded
    }

    /// Reads a session path, including legacy UTF-8 strings. Invalid encodings
    /// remain invalid paths instead of silently naming a different filesystem item.
    #[must_use]
    pub fn from_storage_string(text: &str) -> Self {
        #[cfg(unix)]
        if let Some(hex) = text.strip_prefix("\0native:")
            && !hex.is_empty()
            && hex.len() % 2 == 0
            && hex.is_ascii()
        {
            use std::os::unix::ffi::OsStringExt;
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&hex[index..index + 2], 16))
                .collect::<Result<Vec<_>, _>>();
            if let Ok(bytes) = bytes
                && !bytes.contains(&0)
            {
                return Self(PathBuf::from(std::ffi::OsString::from_vec(bytes)));
            }
        }
        Self::from(text)
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

#[cfg(all(test, unix))]
mod storage_tests {
    use super::*;
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    #[test]
    fn storage_round_trips_native_bytes_without_changing_legacy_paths() {
        for value in [
            b"/tmp/name-\xff".as_slice(),
            b"/tmp/name-\xfe",
            b"/tmp/valid \xe2\x98\x83",
            b"native:ff",
            b"",
        ] {
            let native = VPath::from(PathBuf::from(OsString::from_vec(value.to_vec())));
            let encoded = native.to_storage_string();
            assert_eq!(VPath::from_storage_string(&encoded), native);
            if let Ok(utf8) = std::str::from_utf8(value) {
                assert_eq!(encoded, utf8);
            }
        }
        for invalid in ["\0native:f", "\0native:gg", "\0native:00", "\0native:é"] {
            assert_eq!(VPath::from_storage_string(invalid), VPath::from(invalid));
        }
    }
}
