use dualpane_core::VPath;
use dualpane_vfs::VfsError;

/// Stable error categories presented by the operation UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobErrorKind {
    PermissionDenied,
    NoSpace,
    NotFound,
    AlreadyExists,
    Unsupported,
    IoError,
}

/// One item-level failure. Jobs retain these and continue when it is safe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobError {
    pub path: VPath,
    pub operation: &'static str,
    pub kind: JobErrorKind,
    pub message: String,
}

impl JobError {
    #[must_use]
    pub fn from_vfs(path: VPath, operation: &'static str, error: &VfsError) -> Self {
        let kind = if matches!(error, VfsError::Unsupported { .. }) {
            JobErrorKind::Unsupported
        } else {
            match error.io_kind() {
                Some(std::io::ErrorKind::PermissionDenied) => JobErrorKind::PermissionDenied,
                Some(std::io::ErrorKind::StorageFull) => JobErrorKind::NoSpace,
                Some(std::io::ErrorKind::NotFound) => JobErrorKind::NotFound,
                Some(std::io::ErrorKind::AlreadyExists) => JobErrorKind::AlreadyExists,
                _ => JobErrorKind::IoError,
            }
        };
        Self {
            path,
            operation,
            kind,
            message: error.to_string(),
        }
    }

    #[must_use]
    pub fn unsupported(path: VPath, operation: &'static str) -> Self {
        Self {
            path,
            operation,
            kind: JobErrorKind::Unsupported,
            message: format!("{operation} is not supported by this filesystem backend"),
        }
    }
}
