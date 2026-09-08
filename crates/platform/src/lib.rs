#![deny(unsafe_op_in_unsafe_fn)]

//! The only crate allowed to contain platform `cfg` branches and audited unsafe code.
//!
//! Every unsafe block added here must be preceded by a `// SAFETY:` comment that
//! explains the invariant it relies on.

#[cfg(not(unix))]
mod portable;
#[cfg(unix)]
mod unix;

pub mod secure_delete;

use std::path::PathBuf;

/// Prepared XDG trash directories and a local deletion timestamp.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTrashLocation {
    pub files: PathBuf,
    pub info: PathBuf,
    pub deletion_date: String,
}

#[cfg(not(unix))]
pub use portable::{
    NativeDirectory, allocate_range, available_space, canonicalize, capabilities, copy_file_range,
    create_dir, create_reflink, create_symlink, create_write, hard_link, metadata, open_read,
    open_write, prepare_trash, preserve_metadata, process_rss_kib, read_directory, read_link,
    recommended_copy_concurrency, reflink, remove, rename, rename_noreplace, set_len, set_mode,
    set_mode_elevated, sparse_ranges, sync_file,
};
#[cfg(unix)]
pub use unix::{
    NativeDirectory, allocate_range, available_space, canonicalize, capabilities, copy_file_range,
    create_dir, create_reflink, create_symlink, create_write, hard_link, metadata, open_read,
    open_write, prepare_trash, preserve_metadata, process_rss_kib, read_directory, read_link,
    recommended_copy_concurrency, reflink, remove, rename, rename_noreplace, set_len, set_mode,
    set_mode_elevated, sparse_ranges, sync_file,
};

/// Returns the platform family selected for this build.
#[must_use]
pub const fn platform_family() -> &'static str {
    if cfg!(unix) {
        "unix"
    } else if cfg!(windows) {
        "windows"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::platform_family;

    #[test]
    fn current_platform_has_a_known_family() {
        assert_ne!(platform_family(), "unknown");
    }
}
