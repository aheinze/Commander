use std::ffi::OsString;
use std::io::Write;

use dualpane_core::{EntryKind, SizeHint, VPath};
use dualpane_vfs::Vfs;

use crate::{
    Conflict, ConflictDecision, JobControl, JobError, JobErrorKind, JobState, ScanOptions,
    TransferOptions, TransferOutcome, TransferProgress, TransferRecord, TrashRecord, copy_plan,
    scan_sources,
};

/// Moves sources with O(1) rename when possible and verified copy-delete otherwise.
///
/// # Errors
///
/// Returns cooperative cancellation; item failures remain in the outcome.
#[allow(clippy::too_many_arguments)]
pub fn move_sources(
    vfs: &dyn Vfs,
    sources: &[VPath],
    destination: &VPath,
    scan_options: ScanOptions,
    transfer_options: TransferOptions,
    control: &JobControl,
    mut progress: impl FnMut(TransferProgress) + Send,
    mut state_changed: impl FnMut(JobState),
    mut ask: impl FnMut(&Conflict) -> Result<ConflictDecision, dualpane_core::Cancelled>,
) -> Result<TransferOutcome, dualpane_core::Cancelled> {
    let mut outcome = TransferOutcome::default();
    let mut paths = Vec::new();
    for source in sources {
        if let Some(name) = source.file_name() {
            paths.push((source.clone(), destination.join_name(name)));
        } else {
            outcome.errors.push(JobError {
                path: source.clone(),
                operation: "move",
                kind: JobErrorKind::Unsupported,
                message: "moving a filesystem root is not supported".to_owned(),
            });
        }
    }
    let moved = move_paths(
        vfs,
        &paths,
        scan_options,
        transfer_options,
        control,
        &mut progress,
        &mut state_changed,
        &mut ask,
    )?;
    crate::transfer::merge_outcome(&mut outcome, moved);
    Ok(outcome)
}

/// Moves to exact paths, including names chosen by Keep Both or history replay.
#[allow(clippy::too_many_arguments)]
pub fn move_paths(
    vfs: &dyn Vfs,
    paths: &[(VPath, VPath)],
    scan_options: ScanOptions,
    transfer_options: TransferOptions,
    control: &JobControl,
    mut progress: impl FnMut(TransferProgress) + Send,
    mut state_changed: impl FnMut(JobState),
    mut ask: impl FnMut(&Conflict) -> Result<ConflictDecision, dualpane_core::Cancelled>,
) -> Result<TransferOutcome, dualpane_core::Cancelled> {
    let mut outcome = TransferOutcome::default();
    // A move job can run a separate copy plan for each root. Keep the user's
    // policy at job scope, and resolve path/metadata-dependent choices anew.
    let mut apply_all = None;
    let mut resolve = |conflict: &Conflict| {
        if let Some(policy) = apply_all {
            let resolution = crate::resolve_conflict(conflict.clone(), policy, || {
                crate::unique_renamed_path(&conflict.destination, |path| {
                    vfs.stat(path, false).is_ok()
                })
            });
            let crate::ConflictResolution::Resolved(action) = resolution else {
                return Err(dualpane_core::Cancelled);
            };
            return Ok(ConflictDecision {
                action,
                apply_to_all: false,
            });
        }
        let decision = ask(conflict)?;
        if decision.apply_to_all {
            apply_all = Some(match decision.action {
                crate::ConflictAction::Create | crate::ConflictAction::Overwrite => {
                    crate::ConflictPolicy::Overwrite
                }
                crate::ConflictAction::Skip => crate::ConflictPolicy::Skip,
                crate::ConflictAction::Rename(_) => crate::ConflictPolicy::Rename,
                crate::ConflictAction::OverwriteIfNewer => crate::ConflictPolicy::OverwriteIfNewer,
            });
        }
        Ok(decision)
    };
    for (source, target) in paths {
        control.checkpoint()?;
        let source_metadata = match vfs.stat(source, false) {
            Ok(metadata) => metadata,
            Err(error) => {
                outcome.errors.push(JobError::from_vfs(
                    source.clone(),
                    "inspect move source",
                    &error,
                ));
                continue;
            }
        };
        let target_metadata = match vfs.stat(target, false) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => None,
            Err(error) => {
                outcome.errors.push(JobError::from_vfs(
                    target.clone(),
                    "inspect move target",
                    &error,
                ));
                continue;
            }
        };
        // Moving onto the same directory entry or inode is a no-op, even through aliases.
        if source == target
            || target_metadata.as_ref().is_some_and(|metadata| {
                source_metadata.identity.is_some() && source_metadata.identity == metadata.identity
            })
        {
            continue;
        }
        let Some(parent) = target.parent() else {
            continue;
        };
        if let Err(error) = crate::transfer::validate_destination(vfs, source, &parent) {
            outcome.errors.push(error);
            continue;
        }
        if scan_options.dereference_symlinks {
            outcome.errors.push(JobError {
                path: source.clone(),
                operation: "move",
                kind: JobErrorKind::Unsupported,
                message: "moves must preserve symbolic links instead of traversing their targets"
                    .to_owned(),
            });
            continue;
        }
        if target_metadata.is_none() {
            let intent = match control.intent("move", Some(source), target) {
                Ok(intent) => intent,
                Err(error) => {
                    outcome.errors.push(error);
                    return Ok(outcome);
                }
            };
            match vfs.rename_noreplace(source, target) {
                Ok(()) => {
                    outcome.completed_items += 1;
                    outcome.transfers.push(TransferRecord {
                        source: source.clone(),
                        destination: target.clone(),
                        metadata: source_metadata.clone(),
                        created: true,
                        source_removed: true,
                    });
                    if let Err(error) = control.applied(intent) {
                        outcome.errors.push(error);
                    }
                    if let Some(journal) = control.journal()
                        && let Some(record) = outcome.transfers.last()
                        && let Err(error) = journal.transfer(record)
                    {
                        outcome.errors.push(error);
                    }
                    progress(TransferProgress {
                        path: source.clone(),
                        bytes: source_metadata.size,
                        items_finished: 1,
                    });
                    let _ = vfs.sync_file(&parent);
                    if let Some(parent) = source.parent() {
                        let _ = vfs.sync_file(&parent);
                    }
                    continue;
                }
                Err(error) if error.raw_os_error() == Some(18) => {}
                Err(error) => {
                    outcome
                        .errors
                        .push(JobError::from_vfs(source.clone(), "rename move", &error));
                    continue;
                }
            }
        }
        let mut plan = scan_sources(
            vfs,
            std::slice::from_ref(source),
            scan_options,
            control,
            |_| {},
        )?;
        let Some(name) = target.file_name() else {
            continue;
        };
        let Some(root_relative) = plan
            .entries
            .first()
            .map(|entry| entry.relative_path.clone())
        else {
            outcome.errors.extend(plan.errors);
            continue;
        };
        for entry in &mut plan.entries {
            let suffix = entry
                .relative_path
                .strip_prefix(&root_relative)
                .expect("scanned descendant");
            entry.relative_path = if suffix.as_os_str().is_empty() {
                std::path::PathBuf::from(name)
            } else {
                std::path::PathBuf::from(name).join(suffix)
            };
        }
        let mut copied = copy_plan(
            vfs,
            &plan,
            &parent,
            TransferOptions {
                durable: true,
                verify: true,
                ..transfer_options
            },
            control,
            &mut progress,
            &mut state_changed,
            &mut resolve,
        )?;
        let scanned = plan
            .entries
            .iter()
            .map(|entry| (&entry.source, entry))
            .collect::<std::collections::HashMap<_, _>>();
        // Delete only the entries actually copied. Never recursively delete the source
        // tree: skipped, unreadable, or newly created children must remain intact.
        copied
            .transfers
            .sort_by_key(|record| std::cmp::Reverse(record.source.as_path().components().count()));
        for record in &mut copied.transfers {
            control.checkpoint()?;
            let Some(entry) = scanned.get(&record.source) else {
                continue;
            };
            if let Err(error) = verify_move_entry(vfs, entry, &record.destination) {
                copied.errors.push(error);
                continue;
            }
            let intent = match control.intent(
                "remove copied source",
                Some(&record.destination),
                &record.source,
            ) {
                Ok(intent) => intent,
                Err(error) => {
                    copied.errors.push(error);
                    break;
                }
            };
            match vfs.remove(&record.source, entry.metadata.kind) {
                Ok(()) => {
                    record.source_removed = true;
                    if let Err(error) = control.applied(intent) {
                        copied.errors.push(error);
                    }
                    if let Some(journal) = control.journal()
                        && let Err(error) = journal.transfer(record)
                    {
                        copied.errors.push(error);
                    }
                }
                Err(error)
                    if entry.metadata.kind == EntryKind::Directory
                        && error.io_kind() == Some(std::io::ErrorKind::DirectoryNotEmpty) => {}
                Err(error) => copied.errors.push(JobError::from_vfs(
                    record.source.clone(),
                    "remove copied move source",
                    &error,
                )),
            }
        }
        crate::transfer::merge_outcome(&mut outcome, copied);
    }
    Ok(outcome)
}

/// Moves each source to the appropriate same-filesystem XDG trash and writes `.trashinfo`.
///
/// # Errors
///
/// Returns cooperative cancellation; item failures remain in the returned outcome.
pub fn trash_sources(
    vfs: &dyn Vfs,
    sources: &[VPath],
    control: &JobControl,
    mut progress: impl FnMut(TransferProgress),
) -> Result<TransferOutcome, dualpane_core::Cancelled> {
    let mut outcome = TransferOutcome::default();
    for source in sources {
        control.checkpoint()?;
        match trash_one(vfs, source, control) {
            Ok(record) => {
                outcome.completed_items = outcome.completed_items.saturating_add(1);
                if let Some(journal) = control.journal()
                    && let Err(error) = journal.trash(&record)
                {
                    outcome.errors.push(error);
                }
                outcome.trash_records.push(record);
                progress(TransferProgress {
                    path: source.clone(),
                    bytes: 0,
                    items_finished: 1,
                });
            }
            Err(error) => outcome.errors.push(error),
        }
    }
    Ok(outcome)
}

/// Permanently deletes only when the caller confirms the exact top-level item count.
///
/// # Errors
///
/// Returns cooperative cancellation; item failures remain in the returned outcome.
pub fn delete_permanently(
    vfs: &dyn Vfs,
    sources: &[VPath],
    confirmed_item_count: usize,
    control: &JobControl,
    mut progress: impl FnMut(TransferProgress),
) -> Result<TransferOutcome, dualpane_core::Cancelled> {
    let mut outcome = TransferOutcome::default();
    if confirmed_item_count != sources.len() {
        outcome.errors.push(JobError {
            path: sources.first().cloned().unwrap_or_else(|| VPath::from(".")),
            operation: "delete permanently",
            kind: JobErrorKind::PermissionDenied,
            message: format!(
                "confirmation named {confirmed_item_count} items, but {} were requested",
                sources.len()
            ),
        });
        return Ok(outcome);
    }
    let plan = scan_sources(vfs, sources, ScanOptions::default(), control, |_| {})?;
    outcome.errors.extend(plan.errors);
    for entry in plan.entries.iter().rev() {
        control.checkpoint()?;
        let intent = match control.intent("delete permanently", None, &entry.source) {
            Ok(intent) => intent,
            Err(error) => {
                outcome.errors.push(error);
                break;
            }
        };
        match vfs.remove(&entry.source, entry.metadata.kind) {
            Ok(()) => {
                outcome.completed_items = outcome.completed_items.saturating_add(1);
                if let Err(error) = control.applied(intent) {
                    outcome.errors.push(error);
                }
                progress(TransferProgress {
                    path: entry.source.clone(),
                    bytes: entry.metadata.size,
                    items_finished: 1,
                });
            }
            Err(error) => outcome.errors.push(JobError::from_vfs(
                entry.source.clone(),
                "delete permanently",
                &error,
            )),
        }
    }
    Ok(outcome)
}

fn trash_one(vfs: &dyn Vfs, source: &VPath, control: &JobControl) -> Result<TrashRecord, JobError> {
    let location = vfs
        .prepare_trash(source)
        .map_err(|error| JobError::from_vfs(source.clone(), "prepare trash", &error))?;
    let (trashed, info) = unique_trash_paths(vfs, source, &location.files, &location.info)?;
    let info_temp = temporary_info_path(vfs, &location.info)?;
    let original = if source.as_path().is_absolute() {
        source.as_path().to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| io_error(source.clone(), "resolve trash path", error))?
            .join(source.as_path())
    };
    let contents = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encode(original.as_os_str().as_encoded_bytes()),
        location.deletion_date
    );
    let mut file = vfs
        .create_write(&info_temp, SizeHint::Exact(contents.len() as u64))
        .map_err(|error| JobError::from_vfs(info_temp.clone(), "create trash info", &error))?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.flush())
    {
        drop(file);
        let _ = vfs.remove(&info_temp, EntryKind::File);
        return Err(io_error(info_temp, "write trash info", error));
    }
    drop(file);
    vfs.sync_file(&info_temp)
        .map_err(|error| JobError::from_vfs(info_temp.clone(), "sync trash info", &error))?;
    vfs.rename_noreplace(&info_temp, &info)
        .map_err(|error| JobError::from_vfs(info.clone(), "publish trash info", &error))?;
    let intent = control.intent("trash", Some(source), &trashed)?;
    if let Err(error) = vfs.rename_noreplace(source, &trashed) {
        let _ = vfs.remove(&info, EntryKind::File);
        return Err(JobError::from_vfs(source.clone(), "move to trash", &error));
    }
    control.applied(intent)?;
    if let Some(parent) = trashed.parent() {
        let _ = vfs.sync_file(&parent);
    }
    Ok(TrashRecord {
        original: source.clone(),
        trashed,
        info,
        info_contents: contents,
    })
}

fn unique_trash_paths(
    vfs: &dyn Vfs,
    source: &VPath,
    files: &VPath,
    info: &VPath,
) -> Result<(VPath, VPath), JobError> {
    let name = source.file_name().ok_or_else(|| JobError {
        path: source.clone(),
        operation: "move to trash",
        kind: JobErrorKind::Unsupported,
        message: "trashing a filesystem root is not supported".to_owned(),
    })?;
    for suffix in 0_u64..10_000 {
        let mut candidate_name = OsString::from(name);
        if suffix > 0 {
            candidate_name.push(format!(".{suffix}"));
        }
        let trashed = files.join_name(&candidate_name);
        let mut info_name = candidate_name;
        info_name.push(".trashinfo");
        let info_path = info.join_name(&info_name);
        if path_is_missing(vfs, &trashed) && path_is_missing(vfs, &info_path) {
            return Ok((trashed, info_path));
        }
    }
    Err(JobError {
        path: source.clone(),
        operation: "move to trash",
        kind: JobErrorKind::AlreadyExists,
        message: "trash collision limit reached".to_owned(),
    })
}

fn temporary_info_path(vfs: &dyn Vfs, info: &VPath) -> Result<VPath, JobError> {
    for suffix in 0_u64..10_000 {
        let name = OsString::from(format!(".dualpane-tmp-trashinfo-{suffix:016x}"));
        let candidate = info.join_name(&name);
        if path_is_missing(vfs, &candidate) {
            return Ok(candidate);
        }
    }
    Err(JobError {
        path: info.clone(),
        operation: "create trash info",
        kind: JobErrorKind::AlreadyExists,
        message: "trash info temporary collision limit reached".to_owned(),
    })
}

fn path_is_missing(vfs: &dyn Vfs, path: &VPath) -> bool {
    vfs.stat(path, false)
        .is_err_and(|error| error.io_kind() == Some(std::io::ErrorKind::NotFound))
}

fn verify_move_entry(
    vfs: &dyn Vfs,
    entry: &crate::PlanEntry,
    destination: &VPath,
) -> Result<(), JobError> {
    let current = vfs
        .stat(&entry.source, false)
        .map_err(|error| JobError::from_vfs(entry.source.clone(), "recheck move source", &error))?;
    if current.kind != entry.metadata.kind
        || current.identity != entry.metadata.identity
        || (current.kind != EntryKind::Directory
            && (current.size != entry.metadata.size || current.modified != entry.metadata.modified))
    {
        return Err(JobError {
            path: entry.source.clone(),
            operation: "verify move source",
            kind: JobErrorKind::IoError,
            message: "source changed during the move; it has been retained".to_owned(),
        });
    }
    let metadata = vfs
        .stat(destination, false)
        .map_err(|error| JobError::from_vfs(destination.clone(), "verify move", &error))?;
    let valid = match entry.metadata.kind {
        EntryKind::File => {
            metadata.kind == EntryKind::File
                && metadata.size == entry.metadata.size
                && metadata.modified == entry.metadata.modified
        }
        EntryKind::Symlink => {
            metadata.kind == EntryKind::Symlink
                && matches!((vfs.read_link(&entry.source), vfs.read_link(destination)), (Ok(source), Ok(target)) if source == target)
        }
        kind => metadata.kind == kind,
    };
    if valid {
        Ok(())
    } else {
        Err(JobError {
            path: destination.clone(),
            operation: "verify move",
            kind: JobErrorKind::IoError,
            message: "destination differs from the scanned source".to_owned(),
        })
    }
}

fn percent_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

fn io_error(path: VPath, operation: &'static str, error: std::io::Error) -> JobError {
    JobError {
        path,
        operation,
        kind: match error.kind() {
            std::io::ErrorKind::PermissionDenied => JobErrorKind::PermissionDenied,
            std::io::ErrorKind::StorageFull => JobErrorKind::NoSpace,
            std::io::ErrorKind::NotFound => JobErrorKind::NotFound,
            std::io::ErrorKind::AlreadyExists => JobErrorKind::AlreadyExists,
            _ => JobErrorKind::IoError,
        },
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dualpane_core::VPath;
    use dualpane_vfs::{LocalFs, Vfs};
    use tempfile::tempdir;

    use super::{delete_permanently, percent_encode, trash_sources};
    use crate::JobControl;

    #[test]
    fn permanent_delete_requires_exact_count_confirmation() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        fs::write(&source, b"payload").expect("source");

        let outcome = delete_permanently(
            &LocalFs,
            &[VPath::from(source.as_path())],
            0,
            &JobControl::new(),
            |_| {},
        )
        .expect("delete");

        assert!(!outcome.errors.is_empty());
        assert!(LocalFs.stat(&VPath::from(source), false).is_ok());
    }

    #[test]
    fn trash_path_encoding_is_byte_exact() {
        assert_eq!(percent_encode(b"/tmp/a b\xff"), "/tmp/a%20b%FF");
    }

    #[test]
    fn trash_writes_xdg_info_and_moves_the_file() {
        const CHILD: &str = "DUALPANE_TRASH_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let source = std::env::var_os("DUALPANE_TRASH_TEST_SOURCE").expect("source env");
            let outcome = trash_sources(
                &LocalFs,
                &[VPath::from(std::path::PathBuf::from(source.clone()))],
                &JobControl::new(),
                |_| {},
            )
            .expect("trash");
            assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
            assert_eq!(outcome.trash_records.len(), 1);
            assert_eq!(
                outcome.trash_records[0].original,
                VPath::from(std::path::PathBuf::from(source))
            );
            return;
        }

        let fixture = tempdir().expect("fixture");
        let data = fixture.path().join("data");
        fs::create_dir(&data).expect("data");
        let source = fixture.path().join("name with space.txt");
        fs::write(&source, b"payload").expect("source");
        let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "operations::tests::trash_writes_xdg_info_and_moves_the_file",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("DUALPANE_TRASH_TEST_SOURCE", &source)
            .env("XDG_DATA_HOME", &data)
            .status()
            .expect("trash child");

        assert!(status.success());
        assert!(!source.exists());
        assert_eq!(
            fs::read(data.join("Trash/files/name with space.txt")).expect("trashed"),
            b"payload"
        );
        let info = fs::read_to_string(data.join("Trash/info/name with space.txt.trashinfo"))
            .expect("trashinfo");
        assert!(info.contains("[Trash Info]"));
        assert!(info.contains("name%20with%20space.txt"));
    }
}
