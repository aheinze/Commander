//! Best-effort file overwriting. This cannot erase snapshots, backups, or remapped blocks.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SecureDeletePlan {
    files: Vec<ReviewedFile>,
    pub bytes: u64,
}

impl SecureDeletePlan {
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|file| file.path.as_path())
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[derive(Clone, Debug)]
struct ReviewedFile {
    path: PathBuf,
    #[cfg(target_os = "linux")]
    snapshot: linux::Snapshot,
}

#[derive(Clone, Debug)]
pub struct SecureDeleteProgress {
    pub path: PathBuf,
    pub bytes_done: u64,
    pub files_done: usize,
    pub verifying: bool,
}

/// Reviews regular local files without changing them. Symlinks and hard links are rejected.
pub fn review_secure_delete(
    paths: &[PathBuf],
    checkpoint: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<SecureDeletePlan> {
    #[cfg(target_os = "linux")]
    return linux::review(paths, checkpoint);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (paths, checkpoint);
        Err(io::Error::other("Secure delete is only supported on Linux"))
    }
}

/// Revalidates the review, overwrites once with zeros, syncs, verifies the logical contents,
/// and unlinks. Cancellation cannot restore overwritten bytes; uncompleted files are retained.
pub fn secure_delete(
    plan: &SecureDeletePlan,
    checkpoint: &mut dyn FnMut() -> io::Result<()>,
    progress: &mut dyn FnMut(SecureDeleteProgress),
) -> io::Result<usize> {
    #[cfg(target_os = "linux")]
    return linux::run(plan, checkpoint, progress);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (plan, checkpoint, progress);
        Err(io::Error::other("Secure delete is only supported on Linux"))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::fs::{File, Metadata};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    use rustix::fs::{self, AtFlags, Mode, OFlags, RenameFlags};

    const CHUNK: usize = 256 * 1024;

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(super) struct Snapshot {
        device: u64,
        inode: u64,
        size: u64,
        modified: (i64, i64),
        changed: (i64, i64),
    }

    impl Snapshot {
        fn read(metadata: &Metadata) -> io::Result<Self> {
            if !metadata.is_file() {
                return Err(io::Error::other(
                    "Select regular files; folders, links, and devices are not supported",
                ));
            }
            if metadata.nlink() != 1 {
                return Err(io::Error::other(
                    "Files with hard links cannot be securely deleted",
                ));
            }
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                size: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            })
        }

        fn same_file(&self, metadata: &Metadata) -> bool {
            metadata.is_file()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
                && metadata.len() == self.size
                && metadata.nlink() == 1
        }
    }

    // Resolve every component from an anchored directory descriptor. No symlink, including
    // an ancestor symlink, may redirect this destructive operation.
    fn parent(path: &Path) -> io::Result<(File, OsString)> {
        if !path.is_absolute() {
            return Err(io::Error::other(
                "Secure delete requires an absolute local file path",
            ));
        }
        let mut parts: Vec<_> = path.components().collect();
        let Some(Component::Normal(name)) = parts.pop() else {
            return Err(io::Error::other("Select a regular file"));
        };
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut directory = File::from(fs::open("/", flags, Mode::empty())?);
        for part in parts {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => {
                    directory = File::from(fs::openat(&directory, name, flags, Mode::empty())?);
                }
                _ => {
                    return Err(io::Error::other(
                        "Use a file path without parent-directory components",
                    ));
                }
            }
        }
        Ok((directory, name.to_owned()))
    }

    fn open(path: &Path, writable: bool) -> io::Result<(File, OsString, File)> {
        let (directory, name) = parent(path)?;
        // NONBLOCK makes FIFOs rejectable without hanging before fstat.
        let flags = if writable {
            OFlags::RDWR
        } else {
            OFlags::RDONLY
        } | OFlags::NOFOLLOW
            | OFlags::CLOEXEC
            | OFlags::NONBLOCK;
        let file = File::from(fs::openat(&directory, &name, flags, Mode::empty())?);
        Snapshot::read(&file.metadata()?)?;
        // Limit operations to ordinary local storage. In particular, proc/sys pseudo-files
        // can look like regular files while writes have unrelated system side effects.
        let kind = fs::fstatfs(&file)?.f_type as u64;
        if !matches!(
            kind,
            0xef53
                | 0x58465342
                | 0x9123683e
                | 0xf2f52010
                | 0x5346544e
                | 0x2011bab0
                | 0x4d44
                | 0x01021994
                | 0x858458f6
                | 0x794c7630
        ) {
            return Err(io::Error::other(
                "This filesystem is not supported for secure delete",
            ));
        }
        Ok((directory, name, file))
    }

    fn contextual(path: &Path, error: impl std::fmt::Display) -> io::Error {
        io::Error::other(format!("{}: {error}", path.display()))
    }

    pub(super) fn review(
        paths: &[PathBuf],
        checkpoint: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<SecureDeletePlan> {
        if paths.is_empty() {
            return Err(io::Error::other("Select local files to securely delete"));
        }
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        let mut bytes = 0_u64;
        for path in paths {
            checkpoint()?;
            let (_, _, file) = open(path, false).map_err(|error| contextual(path, error))?;
            let snapshot = Snapshot::read(&file.metadata()?)?;
            if seen.insert((snapshot.device, snapshot.inode)) {
                bytes = bytes
                    .checked_add(snapshot.size)
                    .ok_or_else(|| io::Error::other("Selection is too large"))?;
                files.push(ReviewedFile {
                    path: path.clone(),
                    snapshot,
                });
            }
        }
        Ok(SecureDeletePlan { files, bytes })
    }

    fn revalidate(target: &ReviewedFile) -> io::Result<(File, OsString, File)> {
        let opened = open(&target.path, true)?;
        if Snapshot::read(&opened.2.metadata()?)? != target.snapshot {
            return Err(io::Error::other(
                "File changed after review; select it again and review the new contents",
            ));
        }
        Ok(opened)
    }

    // Claim the exact reviewed inode in a private directory before overwriting. If a path
    // is replaced during rename, put that replacement back without ever writing to it.
    struct Claimed {
        parent: File,
        directory: File,
        original: OsString,
        temporary: OsString,
        recovery: PathBuf,
        retained: bool,
    }

    impl Claimed {
        fn new(parent: File, original: OsString, path: &Path) -> io::Result<Self> {
            let mut random = [0_u8; 16];
            File::open("/dev/urandom")?.read_exact(&mut random)?;
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let temporary = OsString::from(format!(".commander-erase-{suffix}"));
            fs::mkdirat(&parent, &temporary, Mode::from_raw_mode(0o700))?;
            let directory = match fs::openat(
                &parent,
                &temporary,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => File::from(fd),
                Err(error) => {
                    let _ = fs::unlinkat(&parent, &temporary, AtFlags::REMOVEDIR);
                    return Err(error.into());
                }
            };
            let mut claimed = Self {
                recovery: path.parent().unwrap().join(&temporary).join("contents"),
                parent,
                directory,
                original,
                temporary,
                retained: false,
            };
            fs::renameat_with(
                &claimed.parent,
                &claimed.original,
                &claimed.directory,
                "contents",
                RenameFlags::NOREPLACE,
            )?;
            claimed.retained = true;
            Ok(claimed)
        }

        fn restore(&mut self) -> io::Result<()> {
            if self.retained {
                fs::renameat_with(
                    &self.directory,
                    "contents",
                    &self.parent,
                    &self.original,
                    RenameFlags::NOREPLACE,
                )?;
                self.retained = false;
                self.parent.sync_all()?;
            }
            Ok(())
        }

        fn matches(&self, snapshot: &Snapshot) -> io::Result<bool> {
            let stat = fs::statat(&self.directory, "contents", AtFlags::SYMLINK_NOFOLLOW)?;
            Ok(stat.st_dev == snapshot.device && stat.st_ino == snapshot.inode)
        }
    }

    impl Drop for Claimed {
        fn drop(&mut self) {
            let _ = self.restore();
            if !self.retained {
                let _ = fs::unlinkat(&self.parent, &self.temporary, AtFlags::REMOVEDIR);
            }
        }
    }

    pub(super) fn run(
        plan: &SecureDeletePlan,
        checkpoint: &mut dyn FnMut() -> io::Result<()>,
        progress: &mut dyn FnMut(SecureDeleteProgress),
    ) -> io::Result<usize> {
        // Validate the entire batch before the first write, then each file again on use.
        for target in &plan.files {
            checkpoint()?;
            revalidate(target).map_err(|error| contextual(&target.path, error))?;
        }
        let zeros = vec![0_u8; CHUNK];
        let mut buffer = vec![0_u8; CHUNK];
        let mut bytes_done = 0_u64;
        for (index, target) in plan.files.iter().enumerate() {
            checkpoint()?;
            let result = (|| {
                let (parent, name, mut file) = revalidate(target)?;
                fs::flock(&file, fs::FlockOperation::NonBlockingLockExclusive)?;
                let mut claimed = Claimed::new(parent, name, &target.path)?;
                let result = (|| {
                    if !claimed.matches(&target.snapshot)?
                        || !target.snapshot.same_file(&file.metadata()?)
                    {
                        return Err(io::Error::other(
                            "File changed while preparing deletion; nothing was overwritten",
                        ));
                    }
                    // Renaming changes ctime, but must not hide a content change.
                    let metadata = file.metadata()?;
                    if (metadata.mtime(), metadata.mtime_nsec()) != target.snapshot.modified {
                        return Err(io::Error::other("File contents changed after review"));
                    }
                    claimed.directory.sync_all()?;
                    claimed.parent.sync_all()?;
                    for verifying in [false, true] {
                        file.seek(SeekFrom::Start(0))?;
                        let mut remaining = target.snapshot.size;
                        while remaining > 0 {
                            checkpoint()?;
                            if !target.snapshot.same_file(&file.metadata()?) {
                                return Err(io::Error::other(
                                    "File size or link count changed during deletion",
                                ));
                            }
                            let length = remaining.min(CHUNK as u64) as usize;
                            if verifying {
                                file.read_exact(&mut buffer[..length])?;
                                if buffer[..length].iter().any(|byte| *byte != 0) {
                                    return Err(io::Error::other(
                                        "Overwrite verification failed; file retained",
                                    ));
                                }
                            } else {
                                file.write_all(&zeros[..length])?;
                            }
                            remaining -= length as u64;
                            bytes_done = bytes_done.saturating_add(length as u64);
                            progress(SecureDeleteProgress {
                                path: target.path.clone(),
                                bytes_done,
                                files_done: index,
                                verifying,
                            });
                        }
                        file.sync_all()?;
                    }
                    checkpoint()?;
                    if !claimed.matches(&target.snapshot)?
                        || !target.snapshot.same_file(&file.metadata()?)
                    {
                        return Err(io::Error::other(
                            "File changed before removal; file retained",
                        ));
                    }
                    fs::unlinkat(&claimed.directory, "contents", AtFlags::empty())?;
                    claimed.retained = false;
                    claimed.directory.sync_all()?;
                    claimed.parent.sync_all()?;
                    Ok(())
                })();
                if let Err(error) = result {
                    if let Err(restore) = claimed.restore() {
                        return Err(io::Error::other(format!(
                            "{error}. Retained file: {} (could not restore original name: {restore})",
                            claimed.recovery.display()
                        )));
                    }
                    return Err(error);
                }
                Ok(())
            })();
            result.map_err(|error| contextual(&target.path, error))?;
            progress(SecureDeleteProgress {
                path: target.path.clone(),
                bytes_done,
                files_done: index + 1,
                verifying: false,
            });
        }
        Ok(plan.len())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
