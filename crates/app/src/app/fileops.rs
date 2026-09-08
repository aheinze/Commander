//! Filesystem edits driven from the UI thread's worker callbacks: history replay, batch
//! rename, recursive mode changes, and folder synchronisation.

use super::*;

/// Only complete transfers into new destinations can be reversed without a
/// pre-operation backup. Never infer effects from the requested source names.
pub(super) fn transfer_history(
    kind: JobKind,
    records: Vec<TransferRecord>,
) -> Option<HistoryEntry> {
    if records.is_empty() || records.iter().any(|record| !record.created) {
        return None;
    }
    match kind {
        JobKind::Copy => Some(HistoryEntry::Copy {
            records,
            trashed: Vec::new(),
        }),
        JobKind::Move if records.iter().all(|record| record.source_removed) => {
            Some(HistoryEntry::Move { records })
        }
        _ => None,
    }
}

pub(super) fn apply_history(
    engine: OperationEngine,
    vfs: &dyn Vfs,
    entry: &mut HistoryEntry,
    direction: HistoryDirection,
) -> Result<(), String> {
    match (entry, direction) {
        (
            HistoryEntry::Replaced {
                kind,
                records,
                backups,
                trashed,
            },
            direction,
        ) => apply_replaced_history(&engine, vfs, *kind, records, backups, trashed, direction),
        (HistoryEntry::Copy { records, trashed }, HistoryDirection::Undo) => {
            validate_copy_history(vfs, records, &[])?;
            let roots = transfer_roots(records)
                .into_iter()
                .map(|record| record.destination.clone())
                .collect();
            *trashed = trash_for_history(&engine, vfs, roots)?;
            Ok(())
        }
        (HistoryEntry::Copy { records, trashed }, HistoryDirection::Redo) => {
            validate_copy_history(vfs, records, trashed)?;
            restore_trash(vfs, trashed)?;
            trashed.clear();
            Ok(())
        }
        (HistoryEntry::Move { records }, direction) => {
            let roots = transfer_roots(records);
            let mut paths = Vec::new();
            for record in roots {
                let (source, destination) = match direction {
                    HistoryDirection::Undo => (&record.destination, &record.source),
                    HistoryDirection::Redo => (&record.source, &record.destination),
                };
                ensure_unchanged(vfs, source, &record.metadata)?;
                require_absent(vfs, destination)?;
                paths.push((source.clone(), destination.clone()));
            }
            let mut completed = Vec::new();
            for (source, destination) in &paths {
                if let Err(error) = move_history_path(vfs, source, destination) {
                    let mut recovery = String::new();
                    for (source, destination) in completed.iter().rev() {
                        if let Err(error) = move_history_path(vfs, destination, source) {
                            recovery.push_str(&format!("; rollback failed: {error}"));
                        }
                    }
                    return Err(format!("{error}{recovery}"));
                }
                completed.push((source.clone(), destination.clone()));
            }
            for record in records {
                let path = match direction {
                    HistoryDirection::Undo => &record.source,
                    HistoryDirection::Redo => &record.destination,
                };
                record.metadata = vfs.stat(path, false).map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        (HistoryEntry::Rename { from, to }, direction) => {
            let (source, destination) = match direction {
                HistoryDirection::Undo => (to, from),
                HistoryDirection::Redo => (from, to),
            };
            rename_paths(
                vfs,
                &[(source.clone(), destination.clone())],
                &CancelToken::new(),
            )
            .map(|_| ())
        }
        (HistoryEntry::BatchRename { moves }, direction) => {
            let paths = moves
                .iter()
                .map(|(from, to)| match direction {
                    HistoryDirection::Undo => (to.clone(), from.clone()),
                    HistoryDirection::Redo => (from.clone(), to.clone()),
                })
                .collect::<Vec<_>>();
            rename_paths(vfs, &paths, &CancelToken::new()).map(|_| ())
        }
        (HistoryEntry::Trash { records }, HistoryDirection::Undo) => restore_trash(vfs, records),
        (HistoryEntry::Trash { records }, HistoryDirection::Redo) => {
            let sources = records
                .iter()
                .map(|record| record.original.clone())
                .collect();
            *records = trash_for_history(&engine, vfs, sources)?;
            Ok(())
        }
    }
}

fn move_history_path(vfs: &dyn Vfs, source: &VPath, destination: &VPath) -> Result<(), String> {
    let outcome = dualpane_engine::move_paths(
        vfs,
        &[(source.clone(), destination.clone())],
        ScanOptions::default(),
        TransferOptions {
            conflict_policy: ConflictPolicy::Skip,
            verify: true,
            durable: true,
            parallel: true,
        },
        &JobControl::new(),
        |_| {},
        |_| {},
        |_| Err(dualpane_core::Cancelled),
    )
    .map_err(|_| "History move cancelled".to_owned())?;
    if let Some(error) = outcome.errors.first() {
        return Err(error.message.clone());
    }
    if outcome.transfers.is_empty()
        || outcome
            .transfers
            .iter()
            .any(|record| !record.source_removed)
    {
        return Err(format!(
            "Could not move {source} back to {destination}; a conflict or changed source was retained"
        ));
    }
    Ok(())
}

fn transfer_roots(records: &[TransferRecord]) -> Vec<&TransferRecord> {
    let destinations = records
        .iter()
        .map(|record| record.destination.clone())
        .collect::<BTreeSet<_>>();
    records
        .iter()
        .filter(|record| {
            !record
                .destination
                .as_path()
                .ancestors()
                .skip(1)
                .any(|parent| destinations.contains(parent))
        })
        .collect()
}

fn require_absent(vfs: &dyn Vfs, path: &VPath) -> Result<(), String> {
    match vfs.stat(path, false) {
        Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => Ok(()),
        Err(error) => Err(error.to_string()),
        Ok(_) => Err(format!("Cannot restore {path}: the path already exists")),
    }
}

fn ensure_unchanged(
    vfs: &dyn Vfs,
    path: &VPath,
    expected: &dualpane_core::Metadata,
) -> Result<(), String> {
    let actual = vfs.stat(path, false).map_err(|error| error.to_string())?;
    if actual.kind != expected.kind
        || actual.identity != expected.identity
        || (actual.kind != EntryKind::Directory
            && (actual.size != expected.size || actual.modified != expected.modified))
    {
        return Err(format!(
            "Cannot undo or redo because {path} changed since the operation"
        ));
    }
    Ok(())
}

fn validate_copy_history(
    vfs: &dyn Vfs,
    records: &[TransferRecord],
    trashed: &[TrashRecord],
) -> Result<(), String> {
    if records.is_empty() || records.iter().any(|record| !record.created) {
        return Err("This copy has no safely reversible output".to_owned());
    }
    let actual_path = |path: &VPath| -> VPath {
        for record in trashed {
            if path == &record.original {
                return record.trashed.clone();
            }
            if let Ok(suffix) = path.as_path().strip_prefix(record.original.as_path()) {
                return VPath::from(record.trashed.as_path().join(suffix));
            }
        }
        path.clone()
    };
    let expected = records
        .iter()
        .map(|record| actual_path(&record.destination))
        .collect::<BTreeSet<_>>();
    for record in records {
        ensure_unchanged(vfs, &actual_path(&record.destination), &record.metadata)?;
    }
    let roots = transfer_roots(records)
        .iter()
        .map(|record| actual_path(&record.destination))
        .collect::<Vec<_>>();
    let plan = dualpane_engine::scan_sources(
        vfs,
        &roots,
        ScanOptions::default(),
        &JobControl::new(),
        |_| {},
    )
    .map_err(|_| "History verification cancelled".to_owned())?;
    if !plan.errors.is_empty()
        || plan
            .entries
            .iter()
            .any(|entry| !expected.contains(&entry.source))
        || plan.entries.len() != expected.len()
    {
        return Err(
            "Copied directory contents changed; undo or redo would affect other files".to_owned(),
        );
    }
    Ok(())
}

fn restore_trash(vfs: &dyn Vfs, records: &[TrashRecord]) -> Result<(), String> {
    let paths = records
        .iter()
        .map(|record| (record.trashed.clone(), record.original.clone()))
        .collect::<Vec<_>>();
    rename_paths(vfs, &paths, &CancelToken::new())?;
    for record in records {
        // A leftover info file does not invalidate the completed restoration.
        if let Err(error) = vfs.remove(&record.info, EntryKind::File) {
            tracing::warn!(%error, path = %record.info, "could not remove restored trash metadata");
        }
    }
    Ok(())
}

fn trash_for_history(
    engine: &OperationEngine,
    vfs: &dyn Vfs,
    sources: Vec<VPath>,
) -> Result<Vec<TrashRecord>, String> {
    let summary = engine.spawn_trash(sources).join();
    if summary.state == JobState::Done && summary.outcome.errors.is_empty() {
        return Ok(summary.outcome.trash_records);
    }
    let mut message = summary.outcome.errors.first().map_or_else(
        || format!("History operation ended in {:?}", summary.state),
        |error| error.message.clone(),
    );
    if !summary.outcome.trash_records.is_empty()
        && let Err(error) = restore_trash(vfs, &summary.outcome.trash_records)
    {
        message.push_str(&format!("; rollback failed: {error}"));
    }
    Err(message)
}

pub(super) fn set_mode_tree(
    vfs: &dyn Vfs,
    root: VPath,
    mode: u32,
    recursive: bool,
    cancel: &CancelToken,
) -> Result<usize, String> {
    let mut pending = vec![root];
    let mut changed = 0_usize;
    while let Some(path) = pending.pop() {
        cancel
            .check()
            .map_err(|_| "Permissions update cancelled".to_owned())?;
        let metadata = vfs.stat(&path, false).map_err(|error| error.to_string())?;
        if recursive && metadata.kind == EntryKind::Directory {
            let entries = vfs
                .read_dir(&path, cancel)
                .map_err(|error| error.to_string())?;
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                pending.push(path.join_name(entry.name()));
            }
        }
        vfs.set_mode(&path, mode)
            .map_err(|error| format!("Could not change {path}: {error}"))?;
        changed = changed.saturating_add(1);
    }
    Ok(changed)
}

pub(super) fn batch_rename(
    vfs: &dyn Vfs,
    items: &[(VPath, String)],
    cancel: &CancelToken,
) -> Result<Vec<(VPath, VPath)>, String> {
    let paths = items
        .iter()
        .map(|(source, name)| {
            let name = name.trim();
            if name.is_empty()
                || matches!(name, "." | "..")
                || name.contains('/')
                || name.contains('\0')
            {
                return Err(format!("Invalid name for {source}: {name:?}"));
            }
            let parent = source
                .parent()
                .ok_or_else(|| format!("Cannot rename filesystem root {source}"))?;
            Ok((source.clone(), parent.join_name(OsStr::new(name))))
        })
        .collect::<Result<Vec<_>, String>>()?;
    rename_paths(vfs, &paths, cancel)
}

/// Stage all entries before publishing any name. Used by forward edits, undo,
/// redo, and trash restoration so overlapping names follow the same rules.
pub(super) fn rename_paths(
    vfs: &dyn Vfs,
    paths: &[(VPath, VPath)],
    cancel: &CancelToken,
) -> Result<Vec<(VPath, VPath)>, String> {
    let sources = paths
        .iter()
        .map(|(source, _)| source.clone())
        .collect::<BTreeSet<_>>();
    if sources.len() != paths.len() {
        return Err("An item occurs more than once in this rename".to_owned());
    }
    let mut targets = BTreeSet::new();
    for (source, target) in paths {
        cancel.check().map_err(|_| "Rename cancelled".to_owned())?;
        // Reserve unchanged names as well: they still own their directory entries.
        if !targets.insert(target.clone()) {
            return Err(format!("More than one item would be named {target}"));
        }
        vfs.stat(source, false)
            .map_err(|error| format!("Could not inspect {source}: {error}"))?;
        match vfs.stat(target, false) {
            Ok(_) if !sources.contains(target) => return Err(format!("{target} already exists")),
            Err(error) if error.io_kind() != Some(std::io::ErrorKind::NotFound) => {
                return Err(error.to_string());
            }
            _ => {}
        }
        if sources
            .iter()
            .any(|other| other != source && source.as_path().starts_with(other.as_path()))
        {
            return Err("Cannot rename a directory and one of its descendants together".to_owned());
        }
    }
    let mut reserved = sources.union(&targets).cloned().collect::<BTreeSet<_>>();
    let mut plans = Vec::new();
    for (index, (source, target)) in paths.iter().enumerate() {
        if source == target {
            continue;
        }
        let parent = source
            .parent()
            .ok_or_else(|| format!("Cannot rename filesystem root {source}"))?;
        let mut temporary = None;
        for suffix in 0..1024 {
            let candidate = parent.join_name(OsStr::new(&format!(
                ".omacommander-rename-{}-{index}-{suffix}.tmp",
                std::process::id()
            )));
            if reserved.contains(&candidate) {
                continue;
            }
            match vfs.stat(&candidate, false) {
                Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => {
                    temporary = Some(candidate);
                    break;
                }
                Err(error) => return Err(error.to_string()),
                Ok(_) => {}
            }
        }
        let temporary =
            temporary.ok_or_else(|| "Could not reserve a temporary rename path".to_owned())?;
        reserved.insert(temporary.clone());
        plans.push((source.clone(), target.clone(), temporary));
    }
    for (index, (source, _, temporary)) in plans.iter().enumerate() {
        if let Err(error) = cancel
            .check()
            .map_err(|_| "Rename cancelled".to_owned())
            .and_then(|()| {
                vfs.rename_noreplace(source, temporary)
                    .map_err(|error| error.to_string())
            })
        {
            let rollback = restore_staged(vfs, &plans[..index]);
            return Err(format!("Could not stage rename: {error}{rollback}"));
        }
    }
    for (index, (_, target, temporary)) in plans.iter().enumerate() {
        if let Err(error) = cancel
            .check()
            .map_err(|_| "Rename cancelled".to_owned())
            .and_then(|()| {
                vfs.rename_noreplace(temporary, target)
                    .map_err(|error| error.to_string())
            })
        {
            let mut recovery = String::new();
            // First return every published name to staging. Restoring directly to
            // original names can overwrite another completed item in a cycle.
            for (_, done_target, done_temporary) in plans[..index].iter().rev() {
                if let Err(error) = vfs.rename_noreplace(done_target, done_temporary) {
                    recovery.push_str(&format!("; retained {done_target}: {error}"));
                }
            }
            recovery.push_str(&restore_staged(vfs, &plans));
            return Err(format!("Could not finish rename: {error}{recovery}"));
        }
    }
    Ok(plans
        .into_iter()
        .map(|(source, target, _)| (source, target))
        .collect())
}

fn restore_staged(vfs: &dyn Vfs, plans: &[(VPath, VPath, VPath)]) -> String {
    let mut failures = String::new();
    for (source, _, temporary) in plans.iter().rev() {
        if let Err(error) = vfs.rename_noreplace(temporary, source) {
            failures.push_str(&format!("; recovery item {temporary} -> {source}: {error}"));
        }
    }
    failures
}

pub(super) fn common_parent(paths: &[VPath]) -> Result<VPath, String> {
    let Some(parent) = paths.first().and_then(VPath::parent) else {
        return Err("Cannot replay a move without an original parent".to_owned());
    };
    if paths
        .iter()
        .all(|path| path.parent().as_ref() == Some(&parent))
    {
        Ok(parent)
    } else {
        Err("The moved items came from different parent folders".to_owned())
    }
}

/// An existing merged directory is context, not an output owned by the transfer.
fn owned_records(
    records: &[TransferRecord],
    backups: &[dualpane_engine::journal::BackupRecord],
) -> Vec<TransferRecord> {
    records
        .iter()
        .filter(|record| {
            record.created
                || backups.iter().any(|backup| {
                    record
                        .destination
                        .as_path()
                        .starts_with(backup.original.as_path())
                })
        })
        .cloned()
        .map(|mut record| {
            record.created = true;
            record
        })
        .collect()
}

pub(super) fn transfer_history_with_backups(
    kind: JobKind,
    records: Vec<TransferRecord>,
    backups: Vec<dualpane_engine::journal::BackupRecord>,
) -> Option<HistoryEntry> {
    if records.iter().all(|record| record.created) && backups.is_empty() {
        return transfer_history(kind, records);
    }
    if !matches!(kind, JobKind::Copy | JobKind::Move)
        || records.is_empty()
        || (kind == JobKind::Move && records.iter().any(|record| !record.source_removed))
        || records.iter().any(|record| {
            !record.created
                && record.metadata.kind != EntryKind::Directory
                && !backups.iter().any(|backup| {
                    record
                        .destination
                        .as_path()
                        .starts_with(backup.original.as_path())
                })
        })
        || owned_records(&records, &backups).is_empty()
    {
        return None;
    }
    Some(HistoryEntry::Replaced {
        kind,
        records,
        backups,
        trashed: Vec::new(),
    })
}

fn relocate_all(vfs: &dyn Vfs, paths: &[(VPath, VPath)]) -> Result<(), String> {
    let mut completed: Vec<(VPath, VPath)> = Vec::new();
    for (from, to) in paths {
        if let Err(mut error) = move_history_path(vfs, from, to) {
            for (from, to) in completed.iter().rev() {
                if let Err(rollback) = move_history_path(vfs, to, from) {
                    error.push_str(&format!("; recovery needed: {rollback}"));
                }
            }
            return Err(error);
        }
        completed.push((from.clone(), to.clone()));
    }
    Ok(())
}

fn apply_replaced_history(
    engine: &OperationEngine,
    vfs: &dyn Vfs,
    kind: JobKind,
    records: &mut [TransferRecord],
    backups: &mut [dualpane_engine::journal::BackupRecord],
    trashed: &mut Vec<TrashRecord>,
    direction: HistoryDirection,
) -> Result<(), String> {
    let owned = owned_records(records, backups);
    let roots = transfer_roots(&owned);
    let undo = matches!(direction, HistoryDirection::Undo);
    let backup_moves = backups
        .iter()
        .map(|backup| {
            if undo {
                (backup.backup.clone(), backup.original.clone())
            } else {
                (backup.original.clone(), backup.backup.clone())
            }
        })
        .collect::<Vec<_>>();
    for (backup, (from, to)) in backups.iter().zip(&backup_moves) {
        ensure_unchanged(vfs, from, &backup.metadata)?;
        if !undo {
            require_absent(vfs, to)?;
        }
    }
    if kind == JobKind::Copy {
        validate_copy_history(vfs, &owned, if undo { &[] } else { trashed })?;
        if undo {
            let outputs = roots
                .iter()
                .map(|record| record.destination.clone())
                .collect();
            let removed = trash_for_history(engine, vfs, outputs)?;
            if let Err(mut error) = rename_paths(vfs, &backup_moves, &CancelToken::new()) {
                if let Err(rollback) = restore_trash(vfs, &removed) {
                    error.push_str(&format!("; recovery needed: {rollback}"));
                }
                return Err(error);
            }
            *trashed = removed;
        } else {
            rename_paths(vfs, &backup_moves, &CancelToken::new())?;
            if let Err(mut error) = restore_trash(vfs, trashed) {
                let rollback = backup_moves
                    .iter()
                    .map(|(from, to)| (to.clone(), from.clone()))
                    .collect::<Vec<_>>();
                if let Err(rollback) = rename_paths(vfs, &rollback, &CancelToken::new()) {
                    error.push_str(&format!("; recovery needed: {rollback}"));
                }
                return Err(error);
            }
            trashed.clear();
        }
    } else {
        let mut source_view = records.to_vec();
        if !undo {
            for record in &mut source_view {
                record.destination = record.source.clone();
                record.created = true;
            }
            validate_copy_history(vfs, &source_view, &[])?;
        } else {
            validate_copy_history(vfs, &owned, &[])?;
        }
        let moves = roots
            .iter()
            .map(|record| {
                if undo {
                    (record.destination.clone(), record.source.clone())
                } else {
                    (record.source.clone(), record.destination.clone())
                }
            })
            .collect::<Vec<_>>();
        // Source directory roots merged into existing destination directories are
        // recreated before restoring their children; never reuse an unexpected path.
        let owned_paths = owned
            .iter()
            .map(|record| record.destination.clone())
            .collect::<BTreeSet<_>>();
        let mut merged_dirs = records
            .iter()
            .filter(|record| {
                record.metadata.kind == EntryKind::Directory
                    && !owned_paths.contains(&record.destination)
            })
            .collect::<Vec<_>>();
        merged_dirs.sort_by_key(|record| record.source.as_path().components().count());
        let mut created_dirs = Vec::new();
        if undo {
            for record in records.iter() {
                require_absent(vfs, &record.source)?;
            }
            for record in &merged_dirs {
                if let Err(error) = vfs.create_dir(&record.source) {
                    for path in created_dirs.iter().rev() {
                        let _ = vfs.remove(path, EntryKind::Directory);
                    }
                    return Err(error.to_string());
                }
                created_dirs.push(record.source.clone());
            }
        } else {
            rename_paths(vfs, &backup_moves, &CancelToken::new())?;
        }
        if let Err(mut error) = relocate_all(vfs, &moves) {
            for path in created_dirs.iter().rev() {
                let _ = vfs.remove(path, EntryKind::Directory);
            }
            if !undo {
                let rollback = backup_moves
                    .iter()
                    .map(|(from, to)| (to.clone(), from.clone()))
                    .collect::<Vec<_>>();
                if let Err(rollback) = rename_paths(vfs, &rollback, &CancelToken::new()) {
                    error.push_str(&format!("; recovery needed: {rollback}"));
                }
            }
            return Err(error);
        }
        if undo {
            if let Err(mut error) = rename_paths(vfs, &backup_moves, &CancelToken::new()) {
                let rollback = moves
                    .iter()
                    .rev()
                    .map(|(from, to)| (to.clone(), from.clone()))
                    .collect::<Vec<_>>();
                if let Err(rollback) = relocate_all(vfs, &rollback) {
                    error.push_str(&format!("; recovery needed: {rollback}"));
                }
                for path in created_dirs.iter().rev() {
                    let _ = vfs.remove(path, EntryKind::Directory);
                }
                return Err(error);
            }
        } else {
            for record in merged_dirs.iter().rev() {
                vfs.remove(&record.source, EntryKind::Directory)
                    .map_err(|error| {
                        format!(
                            "Files moved successfully, but {} could not be removed: {error}",
                            record.source
                        )
                    })?;
            }
        }
        if undo {
            for record in merged_dirs.iter().rev() {
                vfs.preserve_metadata(&record.destination, &record.source, &record.metadata)
                    .map_err(|error| error.to_string())?;
            }
        }
        for record in records.iter_mut() {
            let path = if undo {
                &record.source
            } else {
                &record.destination
            };
            record.metadata = vfs.stat(path, false).map_err(|error| error.to_string())?;
        }
    }
    for backup in backups {
        let path = if undo {
            &backup.original
        } else {
            &backup.backup
        };
        backup.metadata = vfs.stat(path, false).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "fileops_tests.rs"]
mod tests;
