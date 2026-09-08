use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::SeekFrom;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, atomic::AtomicUsize};

use dualpane_core::{EntryKind, SizeHint, VPath};
use dualpane_vfs::{Vfs, VfsError};

use crate::{
    Conflict, ConflictAction, ConflictDecision, ConflictPolicy, ConflictResolution, JobControl,
    JobError, JobErrorKind, JobState, PlanEntry, ScanPlan, resolve_conflict, unique_renamed_path,
};

const SMALL_BUFFER: usize = 256 * 1024;
const NORMAL_BUFFER: usize = 1024 * 1024;
const LARGE_BUFFER: usize = 4 * 1024 * 1024;
const LARGE_FILE: u64 = 256 * 1024 * 1024;
const HUGE_FILE: u64 = 1024 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

/// Effective data path used for a copied item.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CopyMethod {
    Reflink,
    CopyFileRange,
    Sparse,
    Buffered,
    HardLink,
    Symlink,
}

/// Transfer behavior independent of any UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferOptions {
    pub conflict_policy: ConflictPolicy,
    pub verify: bool,
    pub durable: bool,
    pub parallel: bool,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            conflict_policy: ConflictPolicy::Ask,
            verify: false,
            durable: false,
            parallel: true,
        }
    }
}

/// What is known about an entry's target path before anything is written to it.
///
/// Grouping the pair keeps callers from transposing two same-typed booleans: `overwrite`
/// only has meaning when something may already exist at the target.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TargetState {
    /// An existing object at the target may be replaced.
    overwrite: bool,
    /// Nothing can exist at the target, so existence checks are skipped.
    known_absent: bool,
}

/// Placement facts about one entry, decided by the planner before its copy runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct EntryPlacement {
    /// Record the published path so later hard-link dependents can point at it.
    publish_destination: bool,
    /// The target sits inside a staging tree that is renamed into place at the end.
    target_tree_staged: bool,
    /// A worker batches this entry's progress, so it reports no path of its own.
    defer_progress_path: bool,
}

/// How one regular file is written to its target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileCopy {
    target: TargetState,
    /// The target sits inside a staging tree, so it needs no temporary file of its own.
    tree_staged: bool,
    /// Re-read the copy and compare it against the source before reporting success.
    verify: bool,
    /// Flush the copy and its parent directory to stable storage before reporting success.
    durable: bool,
}

/// Fine-grained progress emitted by the copy loop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferProgress {
    pub path: VPath,
    pub bytes: u64,
    pub items_finished: u64,
}

/// Exact XDG Trash locations needed to undo and redo a successful trash move.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct TrashRecord {
    pub original: VPath,
    pub trashed: VPath,
    pub info: VPath,
    pub info_contents: String,
}

/// One committed destination, used for safe move cleanup and undo eligibility.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct TransferRecord {
    pub source: VPath,
    pub destination: VPath,
    pub metadata: dualpane_core::Metadata,
    /// False for a replacement or a merge into a pre-existing directory.
    pub created: bool,
    pub source_removed: bool,
}

/// Completed work plus recoverable per-item failures and preservation warnings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransferOutcome {
    pub backups: Vec<crate::journal::BackupRecord>,
    pub journal_path: Option<std::path::PathBuf>,
    pub errors: Vec<JobError>,
    pub warnings: Vec<String>,
    pub methods: HashMap<CopyMethod, u64>,
    /// Bytes copied through userspace buffers; reflinks and range copies do not add here.
    pub bytes_read: u64,
    pub completed_items: u64,
    pub effective_concurrency: usize,
    pub trash_records: Vec<TrashRecord>,
    pub transfers: Vec<TransferRecord>,
}

impl TransferOutcome {
    fn used(&mut self, method: CopyMethod) {
        *self.methods.entry(method).or_default() += 1;
    }
}

struct ReadyEntry<'a> {
    index: usize,
    entry: &'a PlanEntry,
    action: ConflictAction,
    destination: VPath,
    placement: EntryPlacement,
}

struct StagedRoot {
    relative_path: std::path::PathBuf,
    staging: VPath,
    destination: VPath,
    overwrite: bool,
}

struct StagingCleanup<'a> {
    vfs: &'a dyn Vfs,
    paths: Vec<VPath>,
}

impl StagingCleanup<'_> {
    fn track(&mut self, path: VPath) {
        self.paths.push(path);
    }
}

impl Drop for StagingCleanup<'_> {
    fn drop(&mut self) {
        for path in &self.paths {
            if let Ok(metadata) = self.vfs.stat(path, false) {
                let _ = remove_tree(self.vfs, path, metadata.kind);
            }
        }
    }
}

/// Copies an immutable scan plan into a destination directory.
///
/// `Ask` conflicts are queued until all non-conflicting items have run. This means a
/// blocked dialog never stalls unrelated files. A response with `apply_to_all` becomes
/// the policy for the remaining queue.
///
/// # Errors
///
/// Returns only cooperative cancellation. Ordinary item errors are accumulated.
#[allow(clippy::too_many_arguments)]
pub fn copy_plan(
    vfs: &dyn Vfs,
    plan: &ScanPlan,
    destination: &VPath,
    options: TransferOptions,
    control: &JobControl,
    mut progress: impl FnMut(TransferProgress) + Send,
    mut state_changed: impl FnMut(JobState),
    mut ask: impl FnMut(&Conflict) -> Result<ConflictDecision, dualpane_core::Cancelled>,
) -> Result<TransferOutcome, dualpane_core::Cancelled> {
    let span = tracing::info_span!(
        "job.copy",
        items = plan.entries.len(),
        bytes = plan.total_bytes
    );
    let _guard = span.enter();
    let mut outcome = TransferOutcome {
        errors: plan.errors.clone(),
        ..TransferOutcome::default()
    };
    let mut destination_by_plan_index = HashMap::<usize, VPath>::new();
    let mut pending = Vec::new();
    let mut deferred_directories = Vec::new();
    let mut excluded = Vec::new();
    let mut redirects = Vec::new();
    let mut ready = Vec::new();
    let mut dependent = Vec::new();
    let mut directories = Vec::new();
    let mut fresh_directories = Vec::<std::path::PathBuf>::new();
    let mut staged_roots = Vec::<StagedRoot>::new();
    let mut staging_cleanup = StagingCleanup {
        vfs,
        paths: Vec::new(),
    };
    let hardlink_origins = plan
        .entries
        .iter()
        .filter_map(|entry| entry.hardlink_to)
        .collect::<HashSet<_>>();

    if let Ok(Some(available)) = vfs.available_space(destination)
        && available < plan.total_bytes
    {
        control.pause();
        state_changed(JobState::Paused);
        control.checkpoint()?;
        state_changed(JobState::Running);
    }

    for (index, entry) in plan.entries.iter().enumerate() {
        control.checkpoint()?;
        if excluded
            .iter()
            .any(|root: &std::path::PathBuf| entry.relative_path.starts_with(root))
        {
            continue;
        }
        if entry.relative_path.components().count() == 1
            && let Err(error) = validate_destination(vfs, &entry.source, destination)
        {
            outcome.errors.push(error);
            excluded.push(entry.relative_path.clone());
            continue;
        }
        if deferred_directories
            .iter()
            .any(|root: &std::path::PathBuf| entry.relative_path.starts_with(root))
        {
            pending.push((index, entry));
            continue;
        }
        let staged_root = staged_roots.iter().rev().find(|root| {
            entry.relative_path != root.relative_path
                && entry.relative_path.starts_with(&root.relative_path)
        });
        let target = entry_target(entry, destination, &staged_roots, &redirects);
        let final_target = target.clone();
        let inside_fresh_directory = fresh_directories.iter().any(|directory| {
            entry.relative_path != *directory && entry.relative_path.starts_with(directory)
        });
        let directory_was_missing = entry.metadata.kind == EntryKind::Directory
            && (inside_fresh_directory
                || vfs
                    .stat(&target, false)
                    .is_err_and(|error| error.io_kind() == Some(std::io::ErrorKind::NotFound)));
        if entry.metadata.kind == EntryKind::Directory
            && vfs
                .stat(&target, false)
                .is_ok_and(|metadata| metadata.kind != EntryKind::Directory)
        {
            deferred_directories.push(entry.relative_path.clone());
            pending.push((index, entry));
            continue;
        }
        let resolution = if inside_fresh_directory {
            Ok(ConflictResolution::Resolved(ConflictAction::Create))
        } else {
            destination_conflict(vfs, entry, &target, options.conflict_policy)
        };
        match resolution {
            Ok(ConflictResolution::Pending(_)) => {
                if entry.metadata.kind == EntryKind::Directory {
                    deferred_directories.push(entry.relative_path.clone());
                }
                pending.push((index, entry));
            }
            Ok(ConflictResolution::Resolved(action)) => {
                if action == ConflictAction::Skip {
                    if entry.metadata.kind == EntryKind::Directory {
                        excluded.push(entry.relative_path.clone());
                    }
                    continue;
                }
                if let ConflictAction::Rename(ref path) = action
                    && entry.metadata.kind == EntryKind::Directory
                {
                    redirects.push((entry.relative_path.clone(), path.clone()));
                }
                let stage_new_root = entry.metadata.kind == EntryKind::Directory
                    && directory_was_missing
                    && !inside_fresh_directory
                    && entry.relative_path.components().count() == 1
                    && matches!(action, ConflictAction::Overwrite | ConflictAction::Create);
                let queued_destination = if stage_new_root {
                    match vacant_temp_path(vfs, &final_target) {
                        Ok(path) => path,
                        Err(error) => {
                            outcome.errors.push(error);
                            continue;
                        }
                    }
                } else {
                    target
                };
                let queued = ReadyEntry {
                    index,
                    action,
                    entry,
                    destination: queued_destination,
                    placement: EntryPlacement {
                        publish_destination: hardlink_origins.contains(&index),
                        target_tree_staged: staged_root.is_some() || stage_new_root,
                        defer_progress_path: false,
                    },
                };
                if entry.metadata.kind == EntryKind::Directory {
                    let errors_before = outcome.errors.len();
                    process_entry(
                        vfs,
                        queued.entry,
                        queued.index,
                        queued.action,
                        &queued.destination,
                        queued.placement,
                        options,
                        control,
                        &mut destination_by_plan_index,
                        &mut directories,
                        &mut outcome,
                        &mut progress,
                    )?;
                    if outcome.errors.len() != errors_before {
                        excluded.push(entry.relative_path.clone());
                    }
                    if outcome.errors.len() == errors_before && directory_was_missing {
                        fresh_directories.push(entry.relative_path.clone());
                    }
                    if outcome.errors.len() == errors_before && stage_new_root {
                        staging_cleanup.track(queued.destination.clone());
                        staged_roots.push(StagedRoot {
                            relative_path: entry.relative_path.clone(),
                            staging: queued.destination,
                            destination: final_target,
                            overwrite: false,
                        });
                    }
                } else if entry.hardlink_to.is_some() {
                    dependent.push(queued);
                } else {
                    ready.push(queued);
                }
            }
            Err(error) => {
                outcome.errors.push(error);
                if entry.metadata.kind == EntryKind::Directory {
                    excluded.push(entry.relative_path.clone());
                }
            }
        }
    }

    process_ready_parallel(
        vfs,
        destination,
        &ready,
        options,
        control,
        &mut destination_by_plan_index,
        &mut outcome,
        &mut progress,
    )?;
    for queued in dependent {
        process_entry(
            vfs,
            queued.entry,
            queued.index,
            queued.action,
            &queued.destination,
            queued.placement,
            options,
            control,
            &mut destination_by_plan_index,
            &mut directories,
            &mut outcome,
            &mut progress,
        )?;
    }

    let mut apply_all: Option<ConflictPolicy> = None;
    for (index, entry) in pending {
        control.checkpoint()?;
        if excluded
            .iter()
            .any(|root| entry.relative_path.starts_with(root))
        {
            continue;
        }
        let target = entry_target(entry, destination, &staged_roots, &redirects);
        let resolution = match destination_conflict(
            vfs,
            entry,
            &target,
            apply_all.unwrap_or(options.conflict_policy),
        ) {
            Ok(resolution) => resolution,
            Err(error) => {
                outcome.errors.push(error);
                if entry.metadata.kind == EntryKind::Directory {
                    excluded.push(entry.relative_path.clone());
                }
                continue;
            }
        };
        let action = match resolution {
            ConflictResolution::Resolved(action) => action,
            ConflictResolution::Pending(conflict) => {
                let decision = ask(&conflict)?;
                if decision.apply_to_all {
                    apply_all = Some(match decision.action {
                        ConflictAction::Overwrite | ConflictAction::Create => {
                            ConflictPolicy::Overwrite
                        }
                        ConflictAction::Skip => ConflictPolicy::Skip,
                        ConflictAction::Rename(_) => ConflictPolicy::Rename,
                        ConflictAction::OverwriteIfNewer => ConflictPolicy::OverwriteIfNewer,
                    });
                }
                match decision.action {
                    ConflictAction::OverwriteIfNewer => {
                        if conflict.source_metadata.modified
                            > conflict.destination_metadata.modified
                        {
                            ConflictAction::Overwrite
                        } else {
                            ConflictAction::Skip
                        }
                    }
                    action => action,
                }
            }
        };
        control.checkpoint()?;
        if action == ConflictAction::Skip {
            if entry.metadata.kind == EntryKind::Directory {
                excluded.push(entry.relative_path.clone());
            }
            continue;
        }
        let actual = match &action {
            ConflictAction::Rename(path) => path.clone(),
            _ => target.clone(),
        };
        if entry.metadata.kind == EntryKind::Directory {
            redirects.push((entry.relative_path.clone(), actual.clone()));
        }
        let before = outcome.errors.len();
        // Stage a whole conflicting directory before replacing the existing object.
        let existing = vfs.stat(&actual, false).ok();
        let stage = entry.metadata.kind == EntryKind::Directory
            && existing
                .as_ref()
                .is_some_and(|metadata| metadata.kind != EntryKind::Directory);
        let working = if stage {
            match vacant_temp_path(vfs, &actual) {
                Ok(path) => path,
                Err(error) => {
                    outcome.errors.push(error);
                    excluded.push(entry.relative_path.clone());
                    continue;
                }
            }
        } else {
            actual.clone()
        };
        process_entry(
            vfs,
            entry,
            index,
            if stage {
                ConflictAction::Create
            } else {
                action
            },
            &working,
            EntryPlacement {
                publish_destination: hardlink_origins.contains(&index),
                target_tree_staged: stage
                    || staged_roots
                        .iter()
                        .any(|root| entry.relative_path.starts_with(&root.relative_path)),
                ..EntryPlacement::default()
            },
            options,
            control,
            &mut destination_by_plan_index,
            &mut directories,
            &mut outcome,
            &mut progress,
        )?;
        if before != outcome.errors.len() && entry.metadata.kind == EntryKind::Directory {
            excluded.push(entry.relative_path.clone());
        } else if stage {
            staging_cleanup.track(working.clone());
            staged_roots.push(StagedRoot {
                relative_path: entry.relative_path.clone(),
                staging: working,
                destination: actual,
                overwrite: true,
            });
        }
    }

    for (source, target, metadata) in directories.into_iter().rev() {
        match vfs.preserve_metadata(&source, &target, &metadata) {
            Ok(warnings) => append_warnings(&mut outcome, &target, warnings),
            Err(error) => outcome.errors.push(JobError::from_vfs(
                target,
                "preserve directory metadata",
                &error,
            )),
        }
    }
    for root in &staged_roots {
        control.checkpoint()?;
        if root.overwrite && !outcome.errors.is_empty() {
            outcome.transfers.retain(|record| {
                !record
                    .destination
                    .as_path()
                    .starts_with(root.staging.as_path())
            });
            continue;
        }
        if let Err(error) = publish_target(
            vfs,
            &root.staging,
            &root.destination,
            root.overwrite,
            control,
        ) {
            outcome.errors.push(error);
            outcome.transfers.retain(|record| {
                !record
                    .destination
                    .as_path()
                    .starts_with(root.staging.as_path())
            });
        } else {
            for record in &mut outcome.transfers {
                if let Ok(suffix) = record
                    .destination
                    .as_path()
                    .strip_prefix(root.staging.as_path())
                {
                    record.destination = VPath::from(root.destination.as_path().join(suffix));
                    if root.overwrite {
                        record.created = false;
                    }
                    if let Some(journal) = control.journal()
                        && let Err(error) = journal.transfer(record)
                    {
                        outcome.errors.push(error);
                    }
                }
            }
        }
    }
    if options.durable {
        for root in &staged_roots {
            if outcome
                .transfers
                .iter()
                .any(|record| record.destination == root.destination)
            {
                for path in [Some(root.destination.clone()), root.destination.parent()]
                    .into_iter()
                    .flatten()
                {
                    if let Err(error) = vfs.sync_file(&path) {
                        outcome.errors.push(JobError::from_vfs(
                            path,
                            "synchronize published directory",
                            &error,
                        ));
                    }
                }
            }
        }
    }
    for record in &mut outcome.transfers {
        if let Ok(metadata) = vfs.stat(&record.destination, false) {
            record.metadata = metadata;
        }
    }
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
fn process_ready_parallel(
    vfs: &dyn Vfs,
    destination: &VPath,
    ready: &[ReadyEntry<'_>],
    options: TransferOptions,
    control: &JobControl,
    destination_by_plan_index: &mut HashMap<usize, VPath>,
    outcome: &mut TransferOutcome,
    progress: &mut (impl FnMut(TransferProgress) + Send),
) -> Result<(), dualpane_core::Cancelled> {
    if ready.is_empty() {
        outcome.effective_concurrency = outcome.effective_concurrency.max(1);
        return Ok(());
    }
    let workers = if options.parallel {
        vfs.recommended_copy_concurrency(destination)
            .saturating_mul(2)
            .clamp(1, 8)
            .min(ready.len())
    } else {
        1
    };
    outcome.effective_concurrency = outcome.effective_concurrency.max(workers);
    if workers == 1 {
        let mut directories = Vec::new();
        for queued in ready {
            control.checkpoint()?;
            process_entry(
                vfs,
                queued.entry,
                queued.index,
                queued.action.clone(),
                &queued.destination,
                queued.placement,
                options,
                control,
                destination_by_plan_index,
                &mut directories,
                outcome,
                progress,
            )?;
        }
        return Ok(());
    }

    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(workers));
    let progress = Mutex::new(progress);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let results = &results;
            let progress = &progress;
            let next = &next;
            scope.spawn(move || {
                let mut local_outcome = TransferOutcome::default();
                let mut local_destinations = HashMap::new();
                let mut local_directories = Vec::new();
                let mut pending_bytes = 0_u64;
                let mut pending_items = 0_u64;
                let mut pending_path = None::<&VPath>;
                let mut last_progress = std::time::Instant::now();
                let mut cancelled = false;
                loop {
                    let ready_index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(queued) = ready.get(ready_index) else {
                        break;
                    };
                    if control.checkpoint().is_err() {
                        cancelled = true;
                        break;
                    }
                    let stream_progress = queued.entry.metadata.size > LARGE_FILE;
                    let result = process_entry(
                        vfs,
                        queued.entry,
                        queued.index,
                        queued.action.clone(),
                        &queued.destination,
                        EntryPlacement {
                            defer_progress_path: true,
                            ..queued.placement
                        },
                        options,
                        control,
                        &mut local_destinations,
                        &mut local_directories,
                        &mut local_outcome,
                        &mut |update| {
                            if stream_progress {
                                let mut callback =
                                    progress.lock().expect("progress mutex poisoned");
                                (**callback)(update);
                            } else {
                                pending_bytes = pending_bytes.saturating_add(update.bytes);
                                pending_items = pending_items.saturating_add(update.items_finished);
                                pending_path = Some(&queued.entry.source);
                            }
                        },
                    );
                    if pending_path.is_some()
                        && (pending_items >= 256
                            || last_progress.elapsed() >= std::time::Duration::from_millis(50))
                    {
                        let mut callback = progress.lock().expect("progress mutex poisoned");
                        (**callback)(TransferProgress {
                            path: pending_path
                                .take()
                                .expect("pending progress has a path")
                                .clone(),
                            bytes: std::mem::take(&mut pending_bytes),
                            items_finished: std::mem::take(&mut pending_items),
                        });
                        last_progress = std::time::Instant::now();
                    }
                    if result.is_err() {
                        cancelled = true;
                        break;
                    }
                }
                if let Some(path) = pending_path.take() {
                    let mut callback = progress.lock().expect("progress mutex poisoned");
                    (**callback)(TransferProgress {
                        path: path.clone(),
                        bytes: pending_bytes,
                        items_finished: pending_items,
                    });
                }
                let result = if cancelled {
                    Err(dualpane_core::Cancelled)
                } else {
                    Ok((local_destinations, local_outcome))
                };
                results
                    .lock()
                    .expect("parallel result mutex poisoned")
                    .push(result);
            });
        }
    });
    let results = results
        .into_inner()
        .expect("parallel result mutex poisoned");
    for result in results {
        let (published, local) = result?;
        destination_by_plan_index.extend(published);
        merge_outcome(outcome, local);
    }
    Ok(())
}

pub(crate) fn merge_outcome(outcome: &mut TransferOutcome, mut local: TransferOutcome) {
    outcome.errors.append(&mut local.errors);
    outcome.warnings.append(&mut local.warnings);
    outcome.transfers.append(&mut local.transfers);
    for (method, count) in local.methods {
        *outcome.methods.entry(method).or_default() += count;
    }
    outcome.bytes_read = outcome.bytes_read.saturating_add(local.bytes_read);
    outcome.completed_items = outcome
        .completed_items
        .saturating_add(local.completed_items);
    outcome.effective_concurrency = outcome
        .effective_concurrency
        .max(local.effective_concurrency);
}

#[allow(clippy::too_many_arguments)]
fn process_entry(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    index: usize,
    action: ConflictAction,
    default_target: &VPath,
    placement: EntryPlacement,
    options: TransferOptions,
    control: &JobControl,
    destination_by_plan_index: &mut HashMap<usize, VPath>,
    directories: &mut Vec<(VPath, VPath, dualpane_core::Metadata)>,
    outcome: &mut TransferOutcome,
    progress: &mut impl FnMut(TransferProgress),
) -> Result<(), dualpane_core::Cancelled> {
    if action == ConflictAction::Skip {
        progress(TransferProgress {
            path: if placement.defer_progress_path {
                VPath::from(std::path::PathBuf::new())
            } else {
                entry.source.clone()
            },
            bytes: entry.metadata.size,
            items_finished: 1,
        });
        return Ok(());
    }
    let rename_only = matches!(action, ConflictAction::Rename(_) | ConflictAction::Create);
    let target_path = match action {
        ConflictAction::Rename(path) => path,
        ConflictAction::Overwrite | ConflictAction::Skip | ConflictAction::Create => {
            default_target.clone()
        }
        ConflictAction::OverwriteIfNewer => {
            unreachable!("conditional action is resolved per conflict")
        }
    };
    let existing = match vfs.stat(&target_path, false) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => None,
        Err(error) => {
            outcome.errors.push(JobError::from_vfs(
                target_path,
                "inspect copy target",
                &error,
            ));
            return Ok(());
        }
    };
    if target_path == entry.source
        || existing.as_ref().is_some_and(|metadata| {
            metadata.identity.is_some() && metadata.identity == entry.metadata.identity
        })
    {
        outcome.errors.push(JobError {
            path: target_path,
            operation: "copy",
            kind: JobErrorKind::AlreadyExists,
            message: "source and destination refer to the same object".to_owned(),
        });
        return Ok(());
    }
    if rename_only && existing.is_some() {
        outcome.errors.push(JobError {
            path: target_path,
            operation: "keep both",
            kind: JobErrorKind::AlreadyExists,
            message: "destination appeared before publication; retry the copy".to_owned(),
        });
        return Ok(());
    }
    let target = TargetState {
        overwrite: existing.is_some() && !rename_only,
        known_absent: existing.is_none(),
    };
    let created = existing.is_none();
    let result = match entry.metadata.kind {
        EntryKind::Directory => {
            copy_directory(vfs, entry, &target_path, target, directories, control).map(|()| 0)
        }
        EntryKind::File => {
            if let Some(first) = entry.hardlink_to
                && let Some(first_target) = destination_by_plan_index.get(&first)
            {
                copy_hardlink(vfs, first_target, &target_path, target, outcome, control).map(|()| 0)
            } else {
                copy_file(
                    vfs,
                    entry,
                    &target_path,
                    FileCopy {
                        target,
                        tree_staged: placement.target_tree_staged,
                        verify: options.verify,
                        durable: options.durable,
                    },
                    control,
                    outcome,
                    progress,
                )
            }
        }
        EntryKind::Symlink => {
            copy_symlink(vfs, entry, &target_path, target, outcome, control).map(|()| 0)
        }
        kind => Err(JobError {
            path: entry.source.clone(),
            operation: "copy special file",
            kind: JobErrorKind::Unsupported,
            message: format!("copying {kind:?} entries is not supported yet"),
        }),
    };
    // Let the caller register a newly created staging directory for cleanup
    // before propagating cancellation at the next checkpoint.
    if entry.metadata.kind != EntryKind::Directory && control.cancel_token().is_cancelled() {
        return Err(dualpane_core::Cancelled);
    }
    match result {
        Ok(final_bytes) => {
            if placement.publish_destination {
                destination_by_plan_index.insert(index, target_path.clone());
            }
            match vfs.stat(&target_path, false) {
                Ok(metadata) => {
                    let record = TransferRecord {
                        source: entry.source.clone(),
                        destination: target_path.clone(),
                        metadata,
                        created,
                        source_removed: false,
                    };
                    if !placement.target_tree_staged
                        && let Some(journal) = control.journal()
                        && let Err(error) = journal.transfer(&record)
                    {
                        outcome.errors.push(error);
                    }
                    outcome.transfers.push(record);
                }
                Err(error) => outcome.errors.push(JobError::from_vfs(
                    target_path.clone(),
                    "record copied destination",
                    &error,
                )),
            }
            outcome.completed_items = outcome.completed_items.saturating_add(1);
            progress(TransferProgress {
                path: if placement.defer_progress_path {
                    VPath::from(std::path::PathBuf::new())
                } else {
                    entry.source.clone()
                },
                bytes: final_bytes,
                items_finished: 1,
            });
        }
        Err(error) => outcome.errors.push(error),
    }
    Ok(())
}

fn destination_conflict(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    destination: &VPath,
    policy: ConflictPolicy,
) -> Result<ConflictResolution, JobError> {
    let destination_metadata = match vfs.stat(destination, false) {
        Ok(metadata) => metadata,
        Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => {
            return Ok(ConflictResolution::Resolved(ConflictAction::Create));
        }
        Err(error) => {
            return Err(JobError::from_vfs(
                destination.clone(),
                "inspect destination",
                &error,
            ));
        }
    };
    if entry.metadata.kind == EntryKind::Directory
        && destination_metadata.kind == EntryKind::Directory
    {
        return Ok(ConflictResolution::Resolved(ConflictAction::Overwrite));
    }
    let conflict = Conflict {
        source: entry.source.clone(),
        destination: destination.clone(),
        source_metadata: entry.metadata.clone(),
        destination_metadata,
    };
    Ok(resolve_conflict(conflict, policy, || {
        unique_renamed_path(destination, |candidate| vfs.stat(candidate, false).is_ok())
    }))
}

fn copy_directory(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    target: &VPath,
    state: TargetState,
    directories: &mut Vec<(VPath, VPath, dualpane_core::Metadata)>,
    control: &JobControl,
) -> Result<(), JobError> {
    let intent = control.intent("create directory", Some(&entry.source), target)?;
    if state.known_absent {
        vfs.create_dir(target)
            .map_err(|error| JobError::from_vfs(target.clone(), "create directory", &error))?;
    } else {
        match vfs.stat(target, false) {
            Ok(metadata) if metadata.kind == EntryKind::Directory => {}
            Ok(metadata) if state.overwrite => {
                if let Some(journal) = control.journal() {
                    let backup = backup_path(target);
                    journal.backup(crate::journal::BackupRecord {
                        original: target.clone(),
                        backup: backup.clone(),
                        metadata: metadata.clone(),
                    })?;
                    vfs.rename_noreplace(target, &backup).map_err(|error| {
                        JobError::from_vfs(target.clone(), "retain replaced object", &error)
                    })?;
                } else {
                    remove_tree(vfs, target, metadata.kind)?;
                }
                vfs.create_dir(target).map_err(|error| {
                    JobError::from_vfs(target.clone(), "create directory", &error)
                })?;
            }
            Ok(_) => unreachable!("non-overwrite conflict was resolved before copy"),
            Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => {
                vfs.create_dir(target).map_err(|error| {
                    JobError::from_vfs(target.clone(), "create directory", &error)
                })?;
            }
            Err(error) => {
                return Err(JobError::from_vfs(
                    target.clone(),
                    "inspect directory target",
                    &error,
                ));
            }
        }
    }
    control.applied(intent)?;
    directories.push((entry.source.clone(), target.clone(), entry.metadata.clone()));
    Ok(())
}

fn copy_file(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    target: &VPath,
    copy: FileCopy,
    control: &JobControl,
    outcome: &mut TransferOutcome,
    progress: &mut impl FnMut(TransferProgress),
) -> Result<u64, JobError> {
    control
        .checkpoint()
        .map_err(|_| cancelled_error(&entry.source))?;
    control.phase(crate::JobPhase::Copying, Some(entry.source.clone()));
    let (working_target, reflinked) = if copy.tree_staged {
        match vfs.create_reflink(&entry.source, target, SizeHint::Exact(entry.metadata.size)) {
            Ok(reflinked) => (target.clone(), reflinked),
            Err(error) => {
                return Err(JobError::from_vfs(
                    target.clone(),
                    "create staged file",
                    &error,
                ));
            }
        }
    } else {
        create_temp_file(vfs, &entry.source, target, entry.metadata.size, control)?
    };
    let result = copy_file_contents(
        vfs,
        entry,
        &working_target,
        reflinked,
        control,
        outcome,
        progress,
    )
    .and_then(|instant_progress| {
        match vfs.preserve_metadata(&entry.source, &working_target, &entry.metadata) {
            Ok(warnings) => append_warnings(outcome, target, warnings),
            Err(error) => {
                return Err(JobError::from_vfs(
                    target.clone(),
                    "preserve file metadata",
                    &error,
                ));
            }
        }
        control
            .checkpoint()
            .map_err(|_| cancelled_error(&entry.source))?;
        control.phase(crate::JobPhase::Finishing, Some(target.clone()));
        if copy.durable {
            vfs.sync_file(&working_target).map_err(|error| {
                JobError::from_vfs(working_target.clone(), "synchronize copy", &error)
            })?;
        }
        if copy.verify {
            verify_copy_controlled(vfs, &entry.source, &working_target, control)?;
        }
        control
            .checkpoint()
            .map_err(|_| cancelled_error(&entry.source))?;
        control.phase(crate::JobPhase::Finishing, Some(target.clone()));
        if !copy.tree_staged {
            publish_target(vfs, &working_target, target, copy.target.overwrite, control)?;
        }
        if copy.durable
            && let Some(parent) = target.parent()
        {
            vfs.sync_file(&parent).map_err(|error| {
                JobError::from_vfs(parent, "synchronize destination directory", &error)
            })?;
        }
        Ok(instant_progress)
    });
    if result.is_err() {
        let _ = vfs.remove(&working_target, EntryKind::File);
    }
    result
}

fn copy_file_contents(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    temp: &VPath,
    reflinked: bool,
    control: &JobControl,
    outcome: &mut TransferOutcome,
    progress: &mut impl FnMut(TransferProgress),
) -> Result<u64, JobError> {
    if reflinked {
        outcome.used(CopyMethod::Reflink);
        return Ok(entry.metadata.size);
    }

    if entry.metadata.allocated_size < entry.metadata.size
        && let Ok(Some(ranges)) = vfs.sparse_ranges(&entry.source, entry.metadata.size)
    {
        copy_sparse(vfs, entry, temp, &ranges, control, outcome, progress)?;
        outcome.used(CopyMethod::Sparse);
        return Ok(0);
    }

    control
        .checkpoint()
        .map_err(|_| cancelled_error(&entry.source))?;
    let mut range_progress = |bytes| {
        // Range-copy implementations call this between bounded kernel chunks.
        let _ = control.checkpoint();
        control.phase(crate::JobPhase::Copying, Some(entry.source.clone()));
        progress(TransferProgress {
            path: entry.source.clone(),
            bytes,
            items_finished: 0,
        });
    };
    let copied = match vfs.copy_file_range(
        &entry.source,
        temp,
        0,
        entry.metadata.size,
        &control.cancel_token(),
        &mut range_progress,
    ) {
        Ok(bytes) => bytes,
        Err(VfsError::Unsupported { .. }) => 0,
        Err(VfsError::Cancelled) => return Err(cancelled_error(&entry.source)),
        Err(error) => {
            return Err(JobError::from_vfs(
                entry.source.clone(),
                "copy file range",
                &error,
            ));
        }
    };
    control
        .checkpoint()
        .map_err(|_| cancelled_error(&entry.source))?;
    if copied == entry.metadata.size {
        outcome.used(CopyMethod::CopyFileRange);
        return Ok(0);
    }
    copy_buffered(vfs, entry, temp, copied, control, outcome, progress)?;
    outcome.used(if copied == 0 {
        CopyMethod::Buffered
    } else {
        CopyMethod::CopyFileRange
    });
    Ok(0)
}

fn copy_sparse(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    temp: &VPath,
    ranges: &[(u64, u64)],
    control: &JobControl,
    outcome: &mut TransferOutcome,
    progress: &mut impl FnMut(TransferProgress),
) -> Result<(), JobError> {
    vfs.set_len(temp, entry.metadata.size)
        .map_err(|error| JobError::from_vfs(temp.clone(), "create sparse file", &error))?;
    let mut source = vfs
        .open_read(&entry.source)
        .map_err(|error| JobError::from_vfs(entry.source.clone(), "open sparse source", &error))?;
    let mut destination = vfs
        .open_write(temp)
        .map_err(|error| JobError::from_vfs(temp.clone(), "open sparse destination", &error))?;
    let mut data_bytes = 0_u64;
    let mut buffer = vec![0_u8; buffer_size(entry.metadata.size)];
    for &(start, end) in ranges {
        control
            .checkpoint()
            .map_err(|_| cancelled_error(&entry.source))?;
        source
            .seek(SeekFrom::Start(start))
            .map_err(|error| io_job_error(entry.source.clone(), "seek sparse source", error))?;
        destination
            .seek(SeekFrom::Start(start))
            .map_err(|error| io_job_error(temp.clone(), "seek sparse destination", error))?;
        if let Err(error) = vfs.allocate_range(temp, start, end.saturating_sub(start))
            && !matches!(error, VfsError::Unsupported { .. })
            && error.io_kind() != Some(std::io::ErrorKind::Unsupported)
        {
            outcome
                .warnings
                .push(format!("{temp}: preallocate data extent: {error}"));
        }
        let copied = copy_stream_range(
            &mut *source,
            &mut *destination,
            end.saturating_sub(start),
            &mut buffer,
            control,
            &entry.source,
            progress,
        )?;
        data_bytes = data_bytes.saturating_add(copied);
    }
    destination
        .flush()
        .map_err(|error| io_job_error(temp.clone(), "flush sparse destination", error))?;
    outcome.bytes_read = outcome.bytes_read.saturating_add(data_bytes);
    let holes = entry.metadata.size.saturating_sub(data_bytes);
    if holes > 0 {
        progress(TransferProgress {
            path: entry.source.clone(),
            bytes: holes,
            items_finished: 0,
        });
    }
    Ok(())
}

fn copy_buffered(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    temp: &VPath,
    offset: u64,
    control: &JobControl,
    outcome: &mut TransferOutcome,
    progress: &mut impl FnMut(TransferProgress),
) -> Result<(), JobError> {
    let mut source = vfs
        .open_read(&entry.source)
        .map_err(|error| JobError::from_vfs(entry.source.clone(), "open source", &error))?;
    // Fresh streams already start at zero. GVfs FTP streams can be readable
    // without supporting seek, including a redundant seek to byte zero.
    if offset > 0 {
        source
            .seek(SeekFrom::Start(offset))
            .map_err(|error| io_job_error(entry.source.clone(), "seek source", error))?;
    }
    let mut destination = vfs
        .open_write(temp)
        .map_err(|error| JobError::from_vfs(temp.clone(), "open destination", &error))?;
    if offset > 0 {
        destination
            .seek(SeekFrom::Start(offset))
            .map_err(|error| io_job_error(temp.clone(), "seek destination", error))?;
    }
    let mut buffer = vec![0_u8; buffer_size(entry.metadata.size)];
    let copied = copy_stream_range(
        &mut *source,
        &mut *destination,
        entry.metadata.size.saturating_sub(offset),
        &mut buffer,
        control,
        &entry.source,
        progress,
    )?;
    destination
        .flush()
        .map_err(|error| io_job_error(temp.clone(), "flush destination", error))?;
    vfs.set_len(temp, entry.metadata.size)
        .map_err(|error| JobError::from_vfs(temp.clone(), "set destination length", &error))?;
    outcome.bytes_read = outcome.bytes_read.saturating_add(copied);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn copy_stream_range(
    source: &mut dyn dualpane_vfs::ReadSeek,
    destination: &mut dyn dualpane_vfs::WriteSeek,
    mut remaining: u64,
    buffer: &mut [u8],
    control: &JobControl,
    path: &VPath,
    progress: &mut impl FnMut(TransferProgress),
) -> Result<u64, JobError> {
    let mut copied = 0_u64;
    while remaining > 0 {
        control.checkpoint().map_err(|_| cancelled_error(path))?;
        let wanted = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let read = source
            .read(&mut buffer[..wanted])
            .map_err(|error| io_job_error(path.clone(), "read source", error))?;
        if read == 0 {
            break;
        }
        destination
            .write_all(&buffer[..read])
            .map_err(|error| io_job_error(path.clone(), "write destination", error))?;
        let bytes = u64::try_from(read).unwrap_or(u64::MAX);
        copied = copied.saturating_add(bytes);
        remaining = remaining.saturating_sub(bytes);
        progress(TransferProgress {
            path: path.clone(),
            bytes,
            items_finished: 0,
        });
    }
    if remaining == 0 {
        Ok(copied)
    } else {
        Err(JobError {
            path: path.clone(),
            operation: "read source",
            kind: JobErrorKind::IoError,
            message: "source ended before its scanned size".to_owned(),
        })
    }
}

fn copy_hardlink(
    vfs: &dyn Vfs,
    first_target: &VPath,
    target: &VPath,
    state: TargetState,
    outcome: &mut TransferOutcome,
    control: &JobControl,
) -> Result<(), JobError> {
    let temp = vacant_temp_path(vfs, target)?;
    control.intent("prepare temporary", None, &temp)?;
    vfs.hard_link(first_target, &temp)
        .map_err(|error| JobError::from_vfs(target.clone(), "recreate hard link", &error))?;
    let result = publish_target(vfs, &temp, target, state.overwrite, control);
    if result.is_err() {
        let _ = vfs.remove(&temp, EntryKind::File);
    }
    result?;
    outcome.used(CopyMethod::HardLink);
    Ok(())
}

fn copy_symlink(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    target: &VPath,
    state: TargetState,
    outcome: &mut TransferOutcome,
    control: &JobControl,
) -> Result<(), JobError> {
    let link_target = vfs
        .read_link(&entry.source)
        .map_err(|error| JobError::from_vfs(entry.source.clone(), "read symbolic link", &error))?;
    let temp = vacant_temp_path(vfs, target)?;
    control.intent("prepare temporary", None, &temp)?;
    vfs.create_symlink(&VPath::from(link_target), &temp)
        .map_err(|error| JobError::from_vfs(target.clone(), "copy symbolic link", &error))?;
    if let Ok(warnings) = vfs.preserve_metadata(&entry.source, &temp, &entry.metadata) {
        append_warnings(outcome, target, warnings);
    }
    let result = publish_target(vfs, &temp, target, state.overwrite, control);
    if result.is_err() {
        let _ = vfs.remove(&temp, EntryKind::Symlink);
    }
    result?;
    outcome.used(CopyMethod::Symlink);
    Ok(())
}

fn create_temp_file(
    vfs: &dyn Vfs,
    source: &VPath,
    target: &VPath,
    size: u64,
    control: &JobControl,
) -> Result<(VPath, bool), JobError> {
    for _ in 0..128 {
        let candidate = temp_path(target);
        control.intent("prepare temporary", Some(source), &candidate)?;
        match vfs.create_reflink(source, &candidate, SizeHint::Exact(size)) {
            Ok(reflinked) => return Ok((candidate, reflinked)),
            Err(error) if error.io_kind() == Some(std::io::ErrorKind::AlreadyExists) => {}
            Err(error) => {
                return Err(JobError::from_vfs(
                    candidate,
                    "create atomic temporary file",
                    &error,
                ));
            }
        }
    }
    Err(JobError {
        path: target.clone(),
        operation: "create atomic temporary file",
        kind: JobErrorKind::AlreadyExists,
        message: "temporary filename collision limit reached".to_owned(),
    })
}

fn vacant_temp_path(vfs: &dyn Vfs, target: &VPath) -> Result<VPath, JobError> {
    for _ in 0..128 {
        let candidate = temp_path(target);
        match vfs.stat(&candidate, false) {
            Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => {
                return Ok(candidate);
            }
            Ok(_) => {}
            Err(error) => {
                return Err(JobError::from_vfs(
                    candidate,
                    "inspect atomic temporary path",
                    &error,
                ));
            }
        }
    }
    Err(JobError {
        path: target.clone(),
        operation: "choose atomic temporary path",
        kind: JobErrorKind::AlreadyExists,
        message: "temporary filename collision limit reached".to_owned(),
    })
}

fn temp_path(target: &VPath) -> VPath {
    let parent = target.parent().unwrap_or_else(|| VPath::from("."));
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let mut name = target
        .file_name()
        .map_or_else(|| OsString::from("item"), OsString::from);
    name.push(format!(".dualpane-tmp-{id:016x}"));
    parent.join_name(&name)
}

fn publish_target(
    vfs: &dyn Vfs,
    working: &VPath,
    target: &VPath,
    overwrite: bool,
    control: &JobControl,
) -> Result<(), JobError> {
    control.checkpoint().map_err(|_| cancelled_error(target))?;
    let intent = control.intent("publish", Some(working), target)?;
    if !overwrite {
        vfs.rename_noreplace(working, target).map_err(|error| {
            JobError::from_vfs(target.clone(), "publish without replacement", &error)
        })?;
        return control.applied(intent);
    }
    if let Some(journal) = control.journal() {
        let old = vfs
            .stat(target, false)
            .map_err(|error| JobError::from_vfs(target.clone(), "inspect replacement", &error))?;
        let backup = backup_path(target);
        journal.backup(crate::journal::BackupRecord {
            original: target.clone(),
            backup: backup.clone(),
            metadata: old.clone(),
        })?;
        // Hard-linking regular originals keeps their publication atomic; other types
        // use a recorded, recoverable rename on the same filesystem.
        let linked = old.kind == EntryKind::File && vfs.hard_link(target, &backup).is_ok();
        if !linked {
            vfs.rename_noreplace(target, &backup).map_err(|error| {
                JobError::from_vfs(target.clone(), "retain replaced object", &error)
            })?;
        }
        if let Some(parent) = target.parent() {
            vfs.sync_file(&parent)
                .map_err(|error| JobError::from_vfs(parent, "sync retained original", &error))?;
        }
        let result = if linked {
            vfs.rename(working, target)
        } else {
            vfs.rename_noreplace(working, target)
        };
        if let Err(error) = result {
            if !linked {
                let _ = vfs.rename_noreplace(&backup, target);
            }
            return Err(JobError::from_vfs(
                target.clone(),
                "publish replacement",
                &error,
            ));
        }
        return control.applied(intent);
    }
    let backup = vacant_temp_path(vfs, target)?;
    let old = vfs
        .stat(target, false)
        .map_err(|error| JobError::from_vfs(target.clone(), "inspect replacement", &error))?;
    let prepared = vfs.stat(working, false).map_err(|error| {
        JobError::from_vfs(working.clone(), "inspect prepared replacement", &error)
    })?;
    if old.kind != EntryKind::Directory && prepared.kind != EntryKind::Directory {
        // Same-type publication remains a single atomic replacement, with no gap
        // during which a crash would hide the original path.
        return vfs.rename(working, target).map_err(|error| {
            JobError::from_vfs(target.clone(), "publish atomic replacement", &error)
        });
    }
    vfs.rename_noreplace(target, &backup)
        .map_err(|error| JobError::from_vfs(target.clone(), "stage replaced object", &error))?;
    if let Err(error) = vfs.rename_noreplace(working, target) {
        let mut failure = JobError::from_vfs(target.clone(), "publish replacement", &error);
        if let Err(restore) = vfs.rename_noreplace(&backup, target) {
            failure
                .message
                .push_str(&format!("; original retained at {backup}: {restore}"));
        }
        return Err(failure);
    }
    remove_tree(vfs, &backup, old.kind)
}

fn entry_target(
    entry: &PlanEntry,
    destination: &VPath,
    staged: &[StagedRoot],
    redirects: &[(std::path::PathBuf, VPath)],
) -> VPath {
    if let Some(root) = staged.iter().rev().find(|root| {
        entry.relative_path != root.relative_path
            && entry.relative_path.starts_with(&root.relative_path)
    }) {
        return VPath::from(
            root.staging.as_path().join(
                entry
                    .relative_path
                    .strip_prefix(&root.relative_path)
                    .expect("descendant"),
            ),
        );
    }
    if let Some((prefix, target)) = redirects.iter().rev().find(|(prefix, _)| {
        entry.relative_path != *prefix && entry.relative_path.starts_with(prefix)
    }) {
        return VPath::from(
            target.as_path().join(
                entry
                    .relative_path
                    .strip_prefix(prefix)
                    .expect("descendant"),
            ),
        );
    }
    VPath::from(destination.as_path().join(&entry.relative_path))
}

/// Refuse recursive self-copies, including destinations reached through symbolic links.
pub(crate) fn validate_destination(
    vfs: &dyn Vfs,
    source: &VPath,
    destination: &VPath,
) -> Result<(), JobError> {
    let metadata = vfs
        .stat(source, false)
        .map_err(|error| JobError::from_vfs(source.clone(), "inspect transfer source", &error))?;
    if metadata.kind != EntryKind::Directory {
        return Ok(());
    }
    if let (Ok(source_path), Ok(destination_path)) =
        (vfs.canonicalize(source), vfs.canonicalize(destination))
        && destination_path
            .as_path()
            .starts_with(source_path.as_path())
    {
        return Err(JobError {
            path: source.clone(),
            operation: "transfer directory",
            kind: JobErrorKind::Unsupported,
            message: "cannot transfer a directory into itself through an alias".to_owned(),
        });
    }
    let mut current = Some(destination.clone());
    while let Some(path) = current {
        let same = path == *source
            || vfs.stat(&path, true).is_ok_and(|other| {
                metadata.identity.is_some() && metadata.identity == other.identity
            });
        if same {
            return Err(JobError {
                path: source.clone(),
                operation: "transfer directory",
                kind: JobErrorKind::Unsupported,
                message: "cannot transfer a directory into itself or a descendant".to_owned(),
            });
        }
        current = path.parent().filter(|parent| *parent != path);
    }
    Ok(())
}

/// Removes a tree without following symlinks.
pub fn remove_tree(vfs: &dyn Vfs, path: &VPath, kind: EntryKind) -> Result<(), JobError> {
    if kind == EntryKind::Directory {
        let cancel = dualpane_core::CancelToken::new();
        let children = vfs
            .read_dir(path, &cancel)
            .map_err(|error| JobError::from_vfs(path.clone(), "list removal target", &error))?;
        for child in children {
            let child = child
                .map_err(|error| JobError::from_vfs(path.clone(), "read removal entry", &error))?;
            let child_path = path.join_name(child.name());
            let child_kind = vfs
                .stat(&child_path, false)
                .map_err(|error| {
                    JobError::from_vfs(child_path.clone(), "stat removal entry", &error)
                })?
                .kind;
            remove_tree(vfs, &child_path, child_kind)?;
        }
    }
    vfs.remove(path, kind)
        .map_err(|error| JobError::from_vfs(path.clone(), "remove", &error))
}

/// Verifies metadata and file contents before publication.
pub fn verify_copy(vfs: &dyn Vfs, source: &VPath, destination: &VPath) -> Result<(), JobError> {
    verify_copy_controlled(vfs, source, destination, &JobControl::new())
}

pub fn verify_copy_controlled(
    vfs: &dyn Vfs,
    source: &VPath,
    destination: &VPath,
    control: &JobControl,
) -> Result<(), JobError> {
    use std::io::Read;
    let before = vfs
        .stat(source, false)
        .map_err(|error| JobError::from_vfs(source.clone(), "verify source", &error))?;
    let copied = vfs
        .stat(destination, false)
        .map_err(|error| JobError::from_vfs(destination.clone(), "verify destination", &error))?;
    let differs = || JobError {
        path: destination.clone(),
        operation: "verify copy",
        kind: JobErrorKind::IoError,
        message: "destination differs from the source or source changed during verification"
            .to_owned(),
    };
    if before.kind != copied.kind
        || before.size != copied.size
        || before.modified != copied.modified
    {
        return Err(differs());
    }
    if before.kind == EntryKind::File {
        let mut source_file = vfs.open_read(source).map_err(|error| {
            JobError::from_vfs(source.clone(), "verify source contents", &error)
        })?;
        let mut copied_file = vfs.open_read(destination).map_err(|error| {
            JobError::from_vfs(destination.clone(), "verify copied contents", &error)
        })?;
        let mut left = vec![0; NORMAL_BUFFER];
        let mut right = vec![0; NORMAL_BUFFER];
        let mut remaining = before.size;
        while remaining > 0 {
            control.checkpoint().map_err(|_| cancelled_error(source))?;
            control.phase(crate::JobPhase::Verifying, Some(source.clone()));
            let count = remaining.min(NORMAL_BUFFER as u64) as usize;
            source_file
                .read_exact(&mut left[..count])
                .map_err(|error| io_job_error(source.clone(), "verify source contents", error))?;
            copied_file
                .read_exact(&mut right[..count])
                .map_err(|error| {
                    io_job_error(destination.clone(), "verify copied contents", error)
                })?;
            if left[..count] != right[..count] {
                return Err(differs());
            }
            remaining -= count as u64;
        }
    }
    let after = vfs
        .stat(source, false)
        .map_err(|error| JobError::from_vfs(source.clone(), "recheck verified source", &error))?;
    if before.identity != after.identity
        || before.size != after.size
        || before.modified != after.modified
    {
        return Err(differs());
    }
    Ok(())
}

fn append_warnings(outcome: &mut TransferOutcome, path: &VPath, warnings: Vec<String>) {
    outcome.warnings.extend(
        warnings
            .into_iter()
            .map(|warning| format!("{path}: {warning}")),
    );
}

fn buffer_size(size: u64) -> usize {
    if size > HUGE_FILE {
        LARGE_BUFFER
    } else if size < LARGE_FILE {
        SMALL_BUFFER
    } else {
        NORMAL_BUFFER
    }
}

fn cancelled_error(path: &VPath) -> JobError {
    JobError {
        path: path.clone(),
        operation: "copy",
        kind: JobErrorKind::IoError,
        message: "operation cancelled".to_owned(),
    }
}

fn io_job_error(path: VPath, operation: &'static str, error: std::io::Error) -> JobError {
    JobError {
        path,
        operation,
        kind: match error.kind() {
            std::io::ErrorKind::PermissionDenied => JobErrorKind::PermissionDenied,
            std::io::ErrorKind::StorageFull => JobErrorKind::NoSpace,
            std::io::ErrorKind::NotFound => JobErrorKind::NotFound,
            std::io::ErrorKind::AlreadyExists => JobErrorKind::AlreadyExists,
            std::io::ErrorKind::Unsupported => JobErrorKind::Unsupported,
            _ => JobErrorKind::IoError,
        },
        message: error.to_string(),
    }
}

fn backup_path(target: &VPath) -> VPath {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    target
        .parent()
        .unwrap_or_else(|| VPath::from("."))
        .join_name(&OsString::from(format!(".commander-undo-{nonce:x}-{id:x}")))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use dualpane_core::VPath;
    use dualpane_vfs::LocalFs;
    use tempfile::tempdir;

    use super::{CopyMethod, TransferOptions, copy_plan, verify_copy};
    use crate::{ConflictAction, ConflictDecision, JobControl, ScanOptions, scan_sources};

    #[test]
    fn copy_preserves_tree_hardlinks_symlinks_and_sparse_shape() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        fs::create_dir(&source).expect("source");
        fs::create_dir(&destination).expect("destination");
        fs::write(source.join("file"), b"payload").expect("file");
        #[cfg(unix)]
        fs::set_permissions(source.join("file"), fs::Permissions::from_mode(0o640))
            .expect("permissions");
        fs::hard_link(source.join("file"), source.join("hardlink")).expect("hardlink");
        #[cfg(unix)]
        std::os::unix::fs::symlink("file", source.join("symlink")).expect("symlink");
        let sparse = fs::File::create(source.join("sparse")).expect("sparse");
        sparse.set_len(8 * 1024 * 1024).expect("sparse length");
        #[cfg(unix)]
        let invalid_name = OsString::from_vec(b"invalid-\xff-name".to_vec());
        #[cfg(unix)]
        fs::write(source.join(&invalid_name), b"native").expect("invalid UTF-8 file");

        let plan = scan_sources(
            &LocalFs,
            &[VPath::from(source.as_path())],
            ScanOptions::default(),
            &JobControl::new(),
            |_| {},
        )
        .expect("scan");
        let outcome = copy_plan(
            &LocalFs,
            &plan,
            &VPath::from(destination.as_path()),
            TransferOptions::default(),
            &JobControl::new(),
            |_| {},
            |_| {},
            |_| {
                Ok(ConflictDecision {
                    action: ConflictAction::Overwrite,
                    apply_to_all: false,
                })
            },
        )
        .expect("copy");

        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        let copied = destination.join("source");
        assert_eq!(fs::read(copied.join("file")).expect("read"), b"payload");
        assert_eq!(
            fs::metadata(copied.join("file")).expect("one").ino(),
            fs::metadata(copied.join("hardlink")).expect("two").ino()
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(copied.join("file")).expect("mode").mode() & 0o777,
            0o640
        );
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(copied.join("symlink")).expect("link"),
            std::path::Path::new("file")
        );
        assert_eq!(
            fs::metadata(copied.join("sparse")).expect("sparse").len(),
            8 * 1024 * 1024
        );
        #[cfg(unix)]
        assert!(copied.join(&invalid_name).exists());
        #[cfg(unix)]
        assert!(
            fs::metadata(copied.join("sparse"))
                .expect("sparse allocation")
                .blocks()
                * 512
                < 8 * 1024 * 1024
        );
        assert!(outcome.methods.contains_key(&CopyMethod::HardLink));
    }

    #[test]
    fn verification_rejects_a_different_size() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        fs::write(&source, b"one").expect("source");
        fs::write(&destination, b"different").expect("destination");

        assert!(verify_copy(&LocalFs, &VPath::from(source), &VPath::from(destination)).is_err());
    }

    #[test]
    fn vanished_source_is_collected_while_other_items_continue() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        fs::create_dir(&source).expect("source");
        fs::create_dir(&destination).expect("destination");
        fs::write(source.join("remain"), b"kept").expect("remain");
        fs::write(source.join("vanish"), b"gone").expect("vanish");
        let plan = scan_sources(
            &LocalFs,
            &[VPath::from(source.as_path())],
            ScanOptions::default(),
            &JobControl::new(),
            |_| {},
        )
        .expect("scan");
        fs::remove_file(source.join("vanish")).expect("remove raced source");

        let outcome = copy_plan(
            &LocalFs,
            &plan,
            &VPath::from(destination.as_path()),
            TransferOptions::default(),
            &JobControl::new(),
            |_| {},
            |_| {},
            |_| {
                Ok(ConflictDecision {
                    action: ConflictAction::Overwrite,
                    apply_to_all: false,
                })
            },
        )
        .expect("copy");

        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(
            fs::read(destination.join("source/remain")).expect("remaining copy"),
            b"kept"
        );
        assert!(!destination.join("source/vanish").exists());
    }

    #[test]
    fn deferred_conflicts_rename_and_overwrite_against_an_existing_tree() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        fs::create_dir(&source).expect("source");
        fs::create_dir_all(destination.join("source")).expect("destination tree");
        fs::write(source.join("renamed"), b"fresh").expect("renamed source");
        fs::write(source.join("replaced"), b"fresh").expect("replaced source");
        fs::write(destination.join("source/renamed"), b"stale").expect("renamed target");
        fs::write(destination.join("source/replaced"), b"stale").expect("replaced target");
        let plan = scan_sources(
            &LocalFs,
            &[VPath::from(source.as_path())],
            ScanOptions::default(),
            &JobControl::new(),
            |_| {},
        )
        .expect("scan");

        let outcome = copy_plan(
            &LocalFs,
            &plan,
            &VPath::from(destination.as_path()),
            TransferOptions::default(),
            &JobControl::new(),
            |_| {},
            |_| {},
            |conflict| {
                let rename = conflict.destination.to_string().ends_with("renamed");
                Ok(ConflictDecision {
                    action: if rename {
                        ConflictAction::Rename(VPath::from(
                            destination.join("source/renamed-copy").as_path(),
                        ))
                    } else {
                        ConflictAction::Overwrite
                    },
                    apply_to_all: false,
                })
            },
        )
        .expect("copy");

        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        // A renamed target leaves the existing file untouched and writes beside it.
        assert_eq!(
            fs::read(destination.join("source/renamed")).expect("kept"),
            b"stale"
        );
        assert_eq!(
            fs::read(destination.join("source/renamed-copy")).expect("renamed"),
            b"fresh"
        );
        // An overwritten target is replaced in place.
        assert_eq!(
            fs::read(destination.join("source/replaced")).expect("replaced"),
            b"fresh"
        );
        assert_eq!(outcome.completed_items, 3);
    }
}
