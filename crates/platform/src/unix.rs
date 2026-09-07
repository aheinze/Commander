use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use dualpane_core::{
    CancelToken, Capabilities, CapabilitySupport, Entry, EntryKind, FileIdentity, Metadata,
    Timestamp,
};
use rustix::fs::{
    self, Advice, AtFlags, CWD, Dir, FileType, Gid, Mode, OFlags, SeekFrom, Stat, Uid, XattrFlags,
};

use crate::NativeTrashLocation;

/// Runs chmod through the system PolicyKit agent without placing a password in process arguments.
pub fn set_mode_elevated(
    path: &Path,
    mode: u32,
    recursive: bool,
    cancel: &CancelToken,
) -> io::Result<()> {
    let mut command = Command::new("pkexec");
    command.arg("/usr/bin/chmod");
    if recursive {
        command.arg("-R");
    }
    let mut child = command
        .arg(format!("{mode:o}"))
        .arg("--")
        .arg(path)
        .spawn()?;
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "elevated permission change cancelled",
            ));
        }
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("administrator authorization failed ({status})"),
                ))
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A `getdents64`-backed native directory stream.
pub struct NativeDirectory {
    directory: Dir,
    device: u64,
}

impl Iterator for NativeDirectory {
    type Item = io::Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let native = self.directory.next()?.map_err(io::Error::from);
            let native = match native {
                Ok(entry) => entry,
                Err(error) => return Some(Err(error)),
            };
            let bytes = native.file_name().to_bytes();
            if matches!(bytes, b"." | b"..") {
                continue;
            }

            let name = OsStr::from_bytes(bytes).to_owned();
            let kind = entry_kind(native.file_type());
            let identity = (native.ino() != 0).then_some(FileIdentity {
                device: self.device,
                inode: native.ino(),
            });
            return Some(Ok(Entry::new(name, kind, identity)));
        }
    }
}

/// Opens a directory using `rustix::fs::Dir`, which uses `getdents64` on Linux.
#[allow(clippy::unnecessary_cast)] // rustix's Dev alias width varies across Unix targets.
pub fn read_directory(path: &Path) -> io::Result<NativeDirectory> {
    let file = fs::openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let stat = fs::fstat(&file).map_err(io::Error::from)?;
    let directory = Dir::new(file).map_err(io::Error::from)?;

    Ok(NativeDirectory {
        directory,
        device: stat.st_dev as u64,
    })
}

/// Fetches full metadata, using `statx` on Linux and `fstatat` elsewhere.
pub fn metadata(path: &Path, follow: bool) -> io::Result<Metadata> {
    metadata_impl(path, follow)
}

/// Changes Unix permission and special mode bits without following UI-side paths.
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777))
}

#[cfg(target_os = "linux")]
fn metadata_impl(path: &Path, follow: bool) -> io::Result<Metadata> {
    let flags = if follow {
        AtFlags::empty()
    } else {
        AtFlags::SYMLINK_NOFOLLOW
    };
    match fs::statx(
        CWD,
        path,
        flags,
        fs::StatxFlags::BASIC_STATS | fs::StatxFlags::BTIME,
    ) {
        Ok(stat) => Ok(Metadata {
            kind: entry_kind(FileType::from_raw_mode(u32::from(stat.stx_mode))),
            identity: Some(FileIdentity {
                device: fs::makedev(stat.stx_dev_major, stat.stx_dev_minor) as u64,
                inode: stat.stx_ino,
            }),
            size: stat.stx_size,
            allocated_size: stat.stx_blocks.saturating_mul(512),
            modified: Some(Timestamp {
                seconds: stat.stx_mtime.tv_sec,
                nanoseconds: stat.stx_mtime.tv_nsec,
            }),
            created: (stat.stx_mask & fs::StatxFlags::BTIME.bits() != 0
                && (stat.stx_btime.tv_sec != 0 || stat.stx_btime.tv_nsec != 0))
                .then_some(Timestamp {
                    seconds: stat.stx_btime.tv_sec,
                    nanoseconds: stat.stx_btime.tv_nsec,
                }),
            accessed: Some(Timestamp {
                seconds: stat.stx_atime.tv_sec,
                nanoseconds: stat.stx_atime.tv_nsec,
            }),
            mode: Some(u32::from(stat.stx_mode)),
            owner: Some(stat.stx_uid),
            group: Some(stat.stx_gid),
            hard_links: Some(u64::from(stat.stx_nlink)),
        }),
        Err(rustix::io::Errno::NOSYS) => metadata_with_fstatat(path, follow),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(not(target_os = "linux"))]
fn metadata_impl(path: &Path, follow: bool) -> io::Result<Metadata> {
    metadata_with_fstatat(path, follow)
}

fn metadata_with_fstatat(path: &Path, follow: bool) -> io::Result<Metadata> {
    let flags = if follow {
        AtFlags::empty()
    } else {
        AtFlags::SYMLINK_NOFOLLOW
    };
    let stat = fs::statat(CWD, path, flags).map_err(io::Error::from)?;
    Ok(metadata_from_stat(&stat))
}

#[allow(clippy::unnecessary_cast)] // rustix's native stat field widths vary by target.
fn metadata_from_stat(stat: &Stat) -> Metadata {
    Metadata {
        kind: entry_kind(FileType::from_raw_mode(stat.st_mode)),
        identity: Some(FileIdentity {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
        }),
        size: u64::try_from(stat.st_size).unwrap_or(0),
        allocated_size: u64::try_from(stat.st_blocks)
            .unwrap_or(0)
            .saturating_mul(512),
        modified: Some(Timestamp {
            seconds: stat.st_mtime,
            nanoseconds: u32::try_from(stat.st_mtime_nsec).unwrap_or(0),
        }),
        created: None,
        accessed: Some(Timestamp {
            seconds: stat.st_atime,
            nanoseconds: u32::try_from(stat.st_atime_nsec).unwrap_or(0),
        }),
        mode: Some(stat.st_mode),
        owner: Some(stat.st_uid),
        group: Some(stat.st_gid),
        hard_links: Some(stat.st_nlink as u64),
    }
}

/// Opens a file for reading.
pub fn open_read(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Creates a new file for writing without overwriting an existing path.
pub fn create_write(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Creates a new destination and attempts `FICLONE` without reopening that destination.
#[cfg(target_os = "linux")]
pub fn create_reflink(source: &Path, destination: &Path) -> io::Result<bool> {
    let source = File::open(source)?;
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = try_ficlone(&source, &output);
    if result.is_err() {
        drop(output);
        let _ = std::fs::remove_file(destination);
    }
    result
}

#[cfg(not(target_os = "linux"))]
pub fn create_reflink(_source: &Path, destination: &Path) -> io::Result<bool> {
    drop(create_write(destination)?);
    Ok(false)
}

/// Opens an existing file without truncation for sparse or resumed copies.
pub fn open_write(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).open(path)
}

/// Creates exactly one directory; the engine owns recursive ordering.
pub fn create_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

/// Reads a symbolic link without interpreting its target.
pub fn read_link(path: &Path) -> io::Result<PathBuf> {
    std::fs::read_link(path)
}

/// Creates a symbolic link while preserving a relative target verbatim.
pub fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    symlink(target, link)
}

/// Creates a hard link.
pub fn hard_link(source: &Path, link: &Path) -> io::Result<()> {
    std::fs::hard_link(source, link)
}

/// Attempts Linux's copy-on-write clone ioctl.
#[cfg(target_os = "linux")]
pub fn reflink(source: &Path, destination: &Path) -> io::Result<bool> {
    let source = File::open(source)?;
    let destination = OpenOptions::new().write(true).open(destination)?;
    try_ficlone(&source, &destination)
}

#[cfg(target_os = "linux")]
fn try_ficlone(source: &File, destination: &File) -> io::Result<bool> {
    match fs::ioctl_ficlone(destination, source) {
        Ok(()) => Ok(true),
        Err(
            rustix::io::Errno::XDEV
            | rustix::io::Errno::OPNOTSUPP
            | rustix::io::Errno::INVAL
            | rustix::io::Errno::NOTTY,
        ) => Ok(false),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn reflink(_source: &Path, _destination: &Path) -> io::Result<bool> {
    Ok(false)
}

/// Copies ranges in bounded chunks so cancellation and progress stay responsive.
#[cfg(target_os = "linux")]
pub fn copy_file_range(
    source: &Path,
    destination: &Path,
    offset: u64,
    length: u64,
    cancel: &dualpane_core::CancelToken,
    progress: &mut dyn FnMut(u64),
) -> io::Result<u64> {
    const CHUNK: usize = 8 * 1024 * 1024;

    let source = File::open(source)?;
    let destination = OpenOptions::new().write(true).open(destination)?;
    let _ = fs::fadvise(&source, offset, None, Advice::Sequential);
    let mut source_offset = offset;
    let mut destination_offset = offset;
    let mut copied = 0_u64;
    while copied < length {
        cancel
            .check()
            .map_err(|_| io::Error::from(io::ErrorKind::Interrupted))?;
        let remaining = length.saturating_sub(copied);
        let chunk = usize::try_from(remaining.min(CHUNK as u64)).unwrap_or(CHUNK);
        match fs::copy_file_range(
            &source,
            Some(&mut source_offset),
            &destination,
            Some(&mut destination_offset),
            chunk,
        ) {
            Ok(0) => break,
            Ok(bytes) => {
                let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
                copied = copied.saturating_add(bytes);
                progress(bytes);
            }
            Err(
                rustix::io::Errno::XDEV
                | rustix::io::Errno::NOSYS
                | rustix::io::Errno::OPNOTSUPP
                | rustix::io::Errno::INVAL,
            ) => break,
            Err(error) => return Err(io::Error::from(error)),
        }
    }
    let _ = fs::fadvise(&source, offset, None, Advice::DontNeed);
    Ok(copied)
}

#[cfg(not(target_os = "linux"))]
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

/// Discovers allocated data ranges with `SEEK_DATA`/`SEEK_HOLE`.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "illumos",
    target_os = "solaris"
))]
pub fn sparse_ranges(path: &Path, length: u64) -> io::Result<Option<Vec<(u64, u64)>>> {
    let file = File::open(path)?;
    let mut ranges = Vec::new();
    let mut cursor = 0_u64;
    while cursor < length {
        let data = match fs::seek(&file, SeekFrom::Data(cursor)) {
            Ok(data) => data,
            Err(rustix::io::Errno::NXIO) => break,
            Err(rustix::io::Errno::INVAL | rustix::io::Errno::OPNOTSUPP) => return Ok(None),
            Err(error) => return Err(io::Error::from(error)),
        };
        let hole = match fs::seek(&file, SeekFrom::Hole(data)) {
            Ok(hole) => hole.min(length),
            Err(rustix::io::Errno::NXIO) => length,
            Err(rustix::io::Errno::INVAL | rustix::io::Errno::OPNOTSUPP) => return Ok(None),
            Err(error) => return Err(io::Error::from(error)),
        };
        if hole <= data {
            return Ok(None);
        }
        ranges.push((data, hole));
        cursor = hole;
    }
    Ok(Some(ranges))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "illumos",
    target_os = "solaris"
)))]
pub fn sparse_ranges(_path: &Path, _length: u64) -> io::Result<Option<Vec<(u64, u64)>>> {
    Ok(None)
}

/// Sets logical length without allocating holes.
pub fn set_len(path: &Path, length: u64) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.set_len(length)
}

#[cfg(target_os = "linux")]
pub fn allocate_range(path: &Path, offset: u64, length: u64) -> io::Result<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    fs::fallocate(&file, fs::FallocateFlags::KEEP_SIZE, offset, length).map_err(io::Error::from)
}

#[cfg(not(target_os = "linux"))]
pub fn allocate_range(_path: &Path, _offset: u64, _length: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "range allocation is unsupported on this platform",
    ))
}

/// Preserves mode, ownership, timestamps, xattrs and POSIX ACL xattrs best-effort.
pub fn preserve_metadata(
    source: &Path,
    destination: &Path,
    metadata: &Metadata,
) -> io::Result<Vec<String>> {
    let mut warnings = Vec::new();
    if metadata.kind == EntryKind::Symlink {
        if let (Some(owner), Some(group)) = (metadata.owner, metadata.group)
            && let Err(error) = fs::chownat(
                CWD,
                destination,
                Some(Uid::from_raw(owner)),
                Some(Gid::from_raw(group)),
                AtFlags::SYMLINK_NOFOLLOW,
            )
        {
            warnings.push(format!("ownership: {error}"));
        }
        preserve_xattrs(source, destination, true, None, &mut warnings);
        if let (Some(accessed), Some(modified)) = (metadata.accessed, metadata.modified) {
            let times = fs::Timestamps {
                last_access: fs::Timespec {
                    tv_sec: accessed.seconds,
                    tv_nsec: i64::from(accessed.nanoseconds),
                },
                last_modification: fs::Timespec {
                    tv_sec: modified.seconds,
                    tv_nsec: i64::from(modified.nanoseconds),
                },
            };
            if let Err(error) = fs::utimensat(CWD, destination, &times, AtFlags::SYMLINK_NOFOLLOW) {
                warnings.push(format!("timestamps: {error}"));
            }
        }
    } else {
        let (current_owner, current_group) = current_process_owner();
        if let (Some(owner), Some(group)) = (metadata.owner, metadata.group)
            && (current_owner != owner || current_group != group)
            && let Err(error) = fs::chownat(
                CWD,
                destination,
                Some(Uid::from_raw(owner)),
                Some(Gid::from_raw(group)),
                AtFlags::empty(),
            )
        {
            warnings.push(format!("ownership: {error}"));
        }
        if let Some(mode) = metadata.mode {
            let requested_mode = mode & 0o7777;
            let creation_mode = current_process_umask().map(|mask| {
                let base = if metadata.kind == EntryKind::Directory {
                    0o777
                } else {
                    0o666
                };
                base & !mask
            });
            if creation_mode != Some(requested_mode)
                && let Err(error) = fs::chmodat(
                    CWD,
                    destination,
                    Mode::from_bits_truncate(mode),
                    AtFlags::empty(),
                )
            {
                warnings.push(format!("mode: {error}"));
            }
        }
        preserve_xattrs(source, destination, false, None, &mut warnings);
        if let (Some(accessed), Some(modified)) = (metadata.accessed, metadata.modified) {
            let times = fs::Timestamps {
                last_access: fs::Timespec {
                    tv_sec: accessed.seconds,
                    tv_nsec: i64::from(accessed.nanoseconds),
                },
                last_modification: fs::Timespec {
                    tv_sec: modified.seconds,
                    tv_nsec: i64::from(modified.nanoseconds),
                },
            };
            if let Err(error) = fs::utimensat(CWD, destination, &times, AtFlags::empty()) {
                warnings.push(format!("timestamps: {error}"));
            }
        }
    }
    Ok(warnings)
}

/// Flushes file contents and metadata to stable storage.
pub fn sync_file(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// Reports bytes available to an unprivileged caller.
pub fn available_space(path: &Path) -> io::Result<Option<u64>> {
    let stat = fs::statvfs(path).map_err(io::Error::from)?;
    Ok(Some(stat.f_bavail.saturating_mul(stat.f_frsize)))
}

/// Selects and securely prepares the correct XDG trash on the source filesystem.
pub fn prepare_trash(source: &Path) -> io::Result<NativeTrashLocation> {
    let source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        std::env::current_dir()?.join(source)
    };
    let source_device = std::fs::symlink_metadata(&source)?.dev();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory is unavailable"))?;
    let home_device = std::fs::metadata(existing_ancestor(&data_home)?)?.dev();
    let trash_root = if home_device == source_device {
        data_home.join("Trash")
    } else {
        let mount_root = mount_root(&source, source_device)?;
        let shared = mount_root.join(".Trash");
        let uid = rustix::process::geteuid().as_raw();
        if shared_trash_is_safe(&shared) {
            shared.join(uid.to_string())
        } else {
            mount_root.join(format!(".Trash-{uid}"))
        }
    };
    let files = trash_root.join("files");
    let info = trash_root.join("info");
    create_private_dir_all(&trash_root)?;
    create_private_dir_all(&files)?;
    create_private_dir_all(&info)?;
    Ok(NativeTrashLocation {
        files,
        info,
        deletion_date: local_deletion_date(),
    })
}

/// Chooses one worker for rotational media and up to eight for solid-state storage.
#[must_use]
pub fn recommended_copy_concurrency(path: &Path) -> usize {
    let fallback = std::thread::available_parallelism().map_or(1, |count| count.get().min(8));
    #[cfg(target_os = "linux")]
    {
        let Ok(stat) = fs::stat(path) else {
            return fallback;
        };
        let major = fs::major(stat.st_dev);
        let minor = fs::minor(stat.st_dev);
        let device = PathBuf::from(format!("/sys/dev/block/{major}:{minor}"));
        if let Ok(device) = std::fs::canonicalize(device) {
            let direct = device.join("queue/rotational");
            let parent = device
                .parent()
                .map(|parent| parent.join("queue/rotational"));
            let rotational = std::fs::read_to_string(&direct)
                .or_else(|_| {
                    parent.map_or_else(
                        || Err(io::Error::from(io::ErrorKind::NotFound)),
                        std::fs::read_to_string,
                    )
                })
                .ok();
            if rotational
                .as_deref()
                .is_some_and(|value| value.trim() == "1")
            {
                return 1;
            }
        }
    }
    fallback
}

/// Renames a native path atomically when the filesystem permits it.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

/// Atomically refuses to replace an existing destination, including dangling links.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
pub fn rename_noreplace(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable",
    ))
}

/// Removes one native filesystem object without following symlinks.
pub fn remove(path: &Path, kind: EntryKind) -> io::Result<()> {
    if kind.is_directory() {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Reports syscall-level support while marking filesystem-specific features as such.
#[must_use]
pub const fn capabilities() -> Capabilities {
    Capabilities {
        stable_identity: CapabilitySupport::Supported,
        atomic_rename: CapabilitySupport::FilesystemDependent,
        hard_links: CapabilitySupport::FilesystemDependent,
        reflink: CapabilitySupport::FilesystemDependent,
        copy_file_range: CapabilitySupport::FilesystemDependent,
        sparse_files: CapabilitySupport::FilesystemDependent,
        extended_attributes: CapabilitySupport::FilesystemDependent,
        file_watching: CapabilitySupport::Supported,
    }
}

/// Reads this process's resident set size on Linux.
#[must_use]
pub fn process_rss_kib() -> Option<u64> {
    process_rss_kib_impl()
}

#[cfg(target_os = "linux")]
fn process_rss_kib_impl() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_ascii_whitespace().nth(1)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
const fn process_rss_kib_impl() -> Option<u64> {
    None
}

fn entry_kind(file_type: FileType) -> EntryKind {
    if file_type.is_file() {
        EntryKind::File
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_fifo() {
        EntryKind::Fifo
    } else if file_type.is_socket() {
        EntryKind::Socket
    } else if file_type.is_char_device() {
        EntryKind::CharacterDevice
    } else if file_type.is_block_device() {
        EntryKind::BlockDevice
    } else {
        EntryKind::Unknown
    }
}

fn preserve_xattrs(
    source: &Path,
    destination: &Path,
    symlink: bool,
    destination_file: Option<&File>,
    warnings: &mut Vec<String>,
) {
    let names_length = match xattr_list_length(source, symlink) {
        Ok(length) => length,
        Err(error) => {
            warnings.push(format!("list xattrs: {error}"));
            return;
        }
    };
    if names_length == 0 {
        return;
    }
    let mut names_buffer = vec![0_u8; names_length];
    let listed = if symlink {
        fs::llistxattr(source, &mut names_buffer)
    } else {
        fs::listxattr(source, &mut names_buffer)
    };
    let Ok(listed) = listed else {
        warnings.push("list xattrs changed while copying".to_owned());
        return;
    };
    names_buffer.truncate(listed);
    for bytes in names_buffer
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = OsStr::from_bytes(bytes);
        let value_length = match xattr_value_length(source, name, symlink) {
            Ok(length) => length,
            Err(error) => {
                warnings.push(format!("size xattr {}: {error}", name.to_string_lossy()));
                continue;
            }
        };
        let mut value_buffer = vec![0_u8; value_length];
        let value = if symlink {
            fs::lgetxattr(source, name, &mut value_buffer)
        } else {
            fs::getxattr(source, name, &mut value_buffer)
        };
        let value_length = match value {
            Ok(length) => length,
            Err(error) => {
                warnings.push(format!("read xattr {}: {error}", name.to_string_lossy()));
                continue;
            }
        };
        value_buffer.truncate(value_length);
        let result = if symlink {
            fs::lsetxattr(destination, name, &value_buffer, XattrFlags::empty())
        } else if let Some(destination_file) = destination_file {
            fs::fsetxattr(destination_file, name, &value_buffer, XattrFlags::empty())
        } else {
            fs::setxattr(destination, name, &value_buffer, XattrFlags::empty())
        };
        if let Err(error) = result {
            warnings.push(format!("write xattr {}: {error}", name.to_string_lossy()));
        }
    }
}

fn xattr_list_length(path: &Path, symlink: bool) -> rustix::io::Result<usize> {
    let mut empty: [u8; 0] = [];
    if symlink {
        fs::llistxattr(path, &mut empty)
    } else {
        fs::listxattr(path, &mut empty)
    }
}

fn xattr_value_length(path: &Path, name: &OsStr, symlink: bool) -> rustix::io::Result<usize> {
    let mut empty: [u8; 0] = [];
    if symlink {
        fs::lgetxattr(path, name, &mut empty)
    } else {
        fs::getxattr(path, name, &mut empty)
    }
}

fn current_process_owner() -> (u32, u32) {
    use std::sync::OnceLock;

    static OWNER: OnceLock<(u32, u32)> = OnceLock::new();
    *OWNER.get_or_init(|| {
        (
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        )
    })
}

fn current_process_umask() -> Option<u32> {
    use std::sync::OnceLock;

    static UMASK: OnceLock<Option<u32>> = OnceLock::new();
    *UMASK.get_or_init(|| {
        #[cfg(target_os = "linux")]
        {
            let status = std::fs::read_to_string("/proc/self/status").ok()?;
            let value = status
                .lines()
                .find_map(|line| line.strip_prefix("Umask:"))?
                .trim();
            u32::from_str_radix(value, 8).ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    })
}

fn existing_ancestor(path: &Path) -> io::Result<PathBuf> {
    let mut current = path;
    loop {
        if current.exists() {
            return Ok(current.to_path_buf());
        }
        current = current
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no existing path ancestor"))?;
    }
}

fn mount_root(source: &Path, device: u64) -> io::Result<PathBuf> {
    let mut current = source
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source has no parent"))?;
    loop {
        let Some(parent) = current.parent() else {
            return Ok(current.to_path_buf());
        };
        if std::fs::metadata(parent)?.dev() != device {
            return Ok(current.to_path_buf());
        }
        if parent == current {
            return Ok(current.to_path_buf());
        }
        current = parent;
    }
}

fn shared_trash_is_safe(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    metadata.is_dir() && metadata.mode() & 0o1002 == 0o1002
}

fn create_private_dir_all(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            if metadata.uid() != rustix::process::geteuid().as_raw() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "trash directory is owned by another user",
                ));
            }
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "trash path is not a directory",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true).mode(0o700).create(path)?;
        }
        Err(error) => return Err(error),
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

fn local_deletion_date() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let raw = libc::time_t::try_from(seconds).unwrap_or(libc::time_t::MAX);
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: `local` points to writable storage for one `tm`, and `raw` lives for the call.
    let result = unsafe { libc::localtime_r(&raw, local.as_mut_ptr()) };
    if result.is_null() {
        return "1970-01-01T00:00:00".to_owned();
    }
    // SAFETY: a non-null `localtime_r` result initialized the supplied `tm`.
    let local = unsafe { local.assume_init() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        local.tm_year + 1900,
        local.tm_mon + 1,
        local.tm_mday,
        local.tm_hour,
        local.tm_min,
        local.tm_sec
    )
}

/// Resolves aliases for transfer ancestry checks.
pub fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::ffi::OsStringExt;

    use tempfile::tempdir;

    use super::read_directory;

    #[test]
    fn directory_stream_preserves_invalid_utf8_names() {
        let fixture = tempdir().expect("fixture");
        let native_name = OsString::from_vec(b"invalid-\xff-name".to_vec());
        fs::write(fixture.path().join(&native_name), b"payload").expect("native filename");

        let entries: Vec<_> = read_directory(fixture.path())
            .expect("directory")
            .collect::<std::io::Result<_>>()
            .expect("entries");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name(), native_name);
    }
}
