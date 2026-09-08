use dualpane_core::{CancelToken, EntryKind, Metadata, VPath};
use dualpane_engine::{
    ConflictAction, ConflictDecision, ConflictPolicy, JobEvent, JobHandle, JobState,
    OperationEngine, ScanOptions, TransferOptions,
};
use dualpane_vfs::Vfs;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompareStatus {
    LeftOnly,
    RightOnly,
    Different,
    Same,
}

#[derive(Clone, Debug)]
pub struct CompareEntry {
    pub relative_path: PathBuf,
    pub status: CompareStatus,
    pub left: Option<Metadata>,
    pub right: Option<Metadata>,
    pub left_digest: Option<String>,
    pub right_digest: Option<String>,
    pub left_link: Option<PathBuf>,
    pub right_link: Option<PathBuf>,
}

pub fn compare_directories(
    vfs: &dyn Vfs,
    left: &VPath,
    right: &VPath,
    cancel: &CancelToken,
) -> Result<Vec<CompareEntry>, String> {
    compare_with_contents(vfs, left, right, false, cancel)
}

pub fn compare_with_contents(
    vfs: &dyn Vfs,
    left: &VPath,
    right: &VPath,
    verify: bool,
    cancel: &CancelToken,
) -> Result<Vec<CompareEntry>, String> {
    let left_entries = collect_tree(vfs, left, cancel)?;
    let right_entries = collect_tree(vfs, right, cancel)?;
    let mut paths: Vec<_> = left_entries
        .keys()
        .chain(right_entries.keys())
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|relative_path| {
            check_cancelled(cancel)?;
            let left_meta = left_entries.get(&relative_path).cloned();
            let right_meta = right_entries.get(&relative_path).cloned();
            let left_path = VPath::from(left.as_path().join(&relative_path));
            let right_path = VPath::from(right.as_path().join(&relative_path));
            let fingerprint = |path: &VPath,
                               metadata: &Option<Metadata>|
             -> Result<(Option<String>, Option<PathBuf>), String> {
                match metadata.as_ref().map(|meta| meta.kind) {
                    Some(EntryKind::File) if verify => {
                        let digest = super::sha256(vfs, path, cancel)?;
                        let current = vfs.stat(path, false).map_err(|error| error.to_string())?;
                        if !same_metadata(metadata.as_ref(), Some(&current)) {
                            return Err(format!(
                                "{path} changed during comparison. Compare again."
                            ));
                        }
                        Ok((Some(digest), None))
                    }
                    Some(EntryKind::Symlink) => Ok((
                        None,
                        Some(vfs.read_link(path).map_err(|error| error.to_string())?),
                    )),
                    _ => Ok((None, None)),
                }
            };
            let (left_digest, left_link) = fingerprint(&left_path, &left_meta)?;
            let (right_digest, right_link) = fingerprint(&right_path, &right_meta)?;
            let status = match (&left_meta, &right_meta) {
                (Some(_), None) => CompareStatus::LeftOnly,
                (None, Some(_)) => CompareStatus::RightOnly,
                (Some(left), Some(right))
                    if left.kind == EntryKind::Directory && right.kind == EntryKind::Directory =>
                {
                    CompareStatus::Same
                }
                (Some(left), Some(right))
                    if left.kind == EntryKind::Symlink && right.kind == EntryKind::Symlink =>
                {
                    if left_link == right_link {
                        CompareStatus::Same
                    } else {
                        CompareStatus::Different
                    }
                }
                (Some(left), Some(right))
                    if verify && left.kind == EntryKind::File && right.kind == EntryKind::File =>
                {
                    if left_digest == right_digest {
                        CompareStatus::Same
                    } else {
                        CompareStatus::Different
                    }
                }
                (Some(left), Some(right))
                    if left.kind != right.kind
                        || left.size != right.size
                        || left.modified != right.modified =>
                {
                    CompareStatus::Different
                }
                (Some(_), Some(_)) => CompareStatus::Same,
                (None, None) => unreachable!("path originated in one of the maps"),
            };
            Ok(CompareEntry {
                relative_path,
                status,
                left: left_meta,
                right: right_meta,
                left_digest,
                right_digest,
                left_link,
                right_link,
            })
        })
        .collect::<Result<Vec<_>, String>>()
}

fn collect_tree(
    vfs: &dyn Vfs,
    root: &VPath,
    cancel: &CancelToken,
) -> Result<BTreeMap<PathBuf, Metadata>, String> {
    collect_tree_checked(vfs, root, cancel, false)
}

fn collect_tree_checked(
    vfs: &dyn Vfs,
    root: &VPath,
    cancel: &CancelToken,
    reject_recovery_files: bool,
) -> Result<BTreeMap<PathBuf, Metadata>, String> {
    let mut result = BTreeMap::new();
    let mut pending = vec![(root.clone(), PathBuf::new())];
    while let Some((directory, relative)) = pending.pop() {
        cancel.check().map_err(|_| "Compare cancelled".to_owned())?;
        let entries = vfs
            .read_dir(&directory, cancel)
            .map_err(|error| format!("Could not compare {directory}: {error}"))?;
        for entry in entries {
            check_cancelled(cancel)?;
            let entry = entry.map_err(|error| error.to_string())?;
            if is_recovery_artifact(entry.name()) {
                if reject_recovery_files {
                    return Err(format!(
                        "{root} contains recovery files. Restore or remove them before mirroring this folder."
                    ));
                }
                continue;
            }
            let path = directory.join_name(entry.name());
            let child_relative = relative.join(Path::new(entry.name()));
            let metadata = vfs.stat(&path, false).map_err(|error| error.to_string())?;
            if metadata.kind == EntryKind::Directory {
                pending.push((path, child_relative.clone()));
            }
            result.insert(child_relative, metadata);
        }
    }
    Ok(result)
}

fn is_recovery_artifact(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let hex = |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    if let Some((_, suffix)) = name.rsplit_once(".dualpane-tmp-") {
        return suffix.len() == 16 && hex(suffix);
    }
    name.strip_prefix(".commander-undo-")
        .and_then(|suffix| suffix.split_once('-'))
        .is_some_and(|(nonce, id)| hex(nonce) && hex(id))
}

fn check_cancelled(cancel: &CancelToken) -> Result<(), String> {
    cancel
        .check()
        .map_err(|_| "Folder operation cancelled".to_owned())
}

fn same_metadata(left: Option<&Metadata>, right: Option<&Metadata>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.kind == right.kind
                && left.size == right.size
                && left.modified == right.modified
                && left.mode == right.mode
                && left.identity == right.identity
        }
        (None, None) => true,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncDirection {
    LeftToRight,
    RightToLeft,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncActionKind {
    Copy,
    Replace,
    CreateDirectory,
    Trash,
    Blocked,
}

impl SyncActionKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Copy => "Copy new",
            Self::Replace => "Replace",
            Self::CreateDirectory => "Create folder",
            Self::Trash => "Move to Trash",
            Self::Blocked => "Resolve type conflict",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SyncAction {
    pub relative_path: PathBuf,
    pub kind: SyncActionKind,
    pub source: Option<Metadata>,
    pub destination: Option<Metadata>,
}

#[derive(Clone, Debug)]
pub struct SyncPlan {
    pub left: VPath,
    pub right: VPath,
    pub entries: Vec<CompareEntry>,
    pub direction: SyncDirection,
    pub verified: bool,
    pub actions: Vec<SyncAction>,
}

impl SyncPlan {
    pub fn roots(&self) -> (&VPath, &VPath) {
        match self.direction {
            SyncDirection::LeftToRight => (&self.left, &self.right),
            SyncDirection::RightToLeft => (&self.right, &self.left),
        }
    }
    pub fn blocked(&self) -> bool {
        self.actions
            .iter()
            .any(|action| action.kind == SyncActionKind::Blocked)
    }
}

pub fn plan_sync(
    left: VPath,
    right: VPath,
    entries: Vec<CompareEntry>,
    direction: SyncDirection,
    mirror: bool,
    verified: bool,
) -> Result<SyncPlan, String> {
    let mut actions = Vec::new();
    let mut extras = Vec::new();
    for entry in &entries {
        if entry.relative_path.as_os_str().is_empty()
            || entry
                .relative_path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err("Comparison contains an invalid relative path".to_owned());
        }
        let (source, destination) = match direction {
            SyncDirection::LeftToRight => (&entry.left, &entry.right),
            SyncDirection::RightToLeft => (&entry.right, &entry.left),
        };
        let kind = match (source, destination) {
            (Some(source), Some(destination)) if source.kind != destination.kind => {
                SyncActionKind::Blocked
            }
            (Some(source), _)
                if !matches!(
                    source.kind,
                    EntryKind::File | EntryKind::Directory | EntryKind::Symlink
                ) =>
            {
                SyncActionKind::Blocked
            }
            (Some(_), Some(_)) if entry.status == CompareStatus::Same => continue,
            (Some(source), None) if source.kind == EntryKind::Directory => {
                SyncActionKind::CreateDirectory
            }
            (Some(_), None) => SyncActionKind::Copy,
            (Some(_), Some(_)) => SyncActionKind::Replace,
            (None, Some(_)) if mirror => {
                extras.push(SyncAction {
                    relative_path: entry.relative_path.clone(),
                    kind: SyncActionKind::Trash,
                    source: None,
                    destination: destination.clone(),
                });
                continue;
            }
            _ => continue,
        };
        actions.push(SyncAction {
            relative_path: entry.relative_path.clone(),
            kind,
            source: source.clone(),
            destination: destination.clone(),
        });
    }
    actions.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    extras.sort_by_key(|action| action.relative_path.components().count());
    let mut roots: Vec<PathBuf> = Vec::new();
    for action in extras {
        if !roots
            .iter()
            .any(|root| action.relative_path.starts_with(root))
        {
            roots.push(action.relative_path.clone());
            actions.push(action);
        }
    }
    Ok(SyncPlan {
        left,
        right,
        entries,
        direction,
        verified,
        actions,
    })
}

/// Apply only a still-current reviewed plan. Copies use staging, verification,
/// durable writes and the engine's retained replacement backups.
pub fn execute_sync_plan(
    vfs: &dyn Vfs,
    engine: &OperationEngine,
    plan: &SyncPlan,
    cancel: &CancelToken,
    mut progress: impl FnMut(usize, &str),
) -> Result<usize, String> {
    check_cancelled(cancel)?;
    if plan.actions.iter().any(|action| {
        action.relative_path.as_os_str().is_empty()
            || action
                .relative_path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
    }) {
        return Err("Sync plan contains an invalid relative path".into());
    }
    if plan.blocked() {
        return Err(
            "Resolve file/folder type conflicts and unsupported entries before syncing".into(),
        );
    }
    let (source_root, destination_root) = plan.roots();
    let source_real = vfs
        .canonicalize(source_root)
        .map_err(|error| error.to_string())?;
    let destination_real = vfs
        .canonicalize(destination_root)
        .map_err(|error| error.to_string())?;
    if source_real
        .as_path()
        .starts_with(destination_real.as_path())
        || destination_real
            .as_path()
            .starts_with(source_real.as_path())
    {
        return Err(
            "Sync requires separate folders; identical or nested folders cannot be synchronized"
                .into(),
        );
    }
    progress(0, "Checking that the reviewed folders have not changed…");
    let current = compare_with_contents(vfs, &plan.left, &plan.right, plan.verified, cancel)?;
    if current.len() != plan.entries.len()
        || !current.iter().zip(&plan.entries).all(|(now, old)| {
            now.relative_path == old.relative_path
                && same_metadata(now.left.as_ref(), old.left.as_ref())
                && same_metadata(now.right.as_ref(), old.right.as_ref())
                && now.left_digest == old.left_digest
                && now.right_digest == old.right_digest
                && now.left_link == old.left_link
                && now.right_link == old.right_link
        })
    {
        return Err("The folders changed after comparison. Compare again before syncing; no changes were applied.".into());
    }
    let mut completed = 0;
    let entries: BTreeMap<_, _> = plan
        .entries
        .iter()
        .map(|entry| (entry.relative_path.as_path(), entry))
        .collect();
    for action in &plan.actions {
        let result = (|| {
            check_cancelled(cancel)?;
            let source = VPath::from(source_root.as_path().join(&action.relative_path));
            let destination = VPath::from(destination_root.as_path().join(&action.relative_path));
            progress(
                completed,
                &format!(
                    "{}: {}",
                    action.kind.label(),
                    action.relative_path.display()
                ),
            );
            for (root, expected) in [
                (source_root, &source_real),
                (destination_root, &destination_real),
            ] {
                if vfs.canonicalize(root).map_err(|error| error.to_string())? != *expected {
                    return Err(format!("{root} changed since review. Compare again."));
                }
            }
            check_parents(vfs, destination_root, &action.relative_path)?;
            let entry = entries
                .get(action.relative_path.as_path())
                .ok_or("Sync action was not part of the comparison")?;
            let (source_digest, destination_digest, source_link, destination_link) =
                match plan.direction {
                    SyncDirection::LeftToRight => (
                        &entry.left_digest,
                        &entry.right_digest,
                        &entry.left_link,
                        &entry.right_link,
                    ),
                    SyncDirection::RightToLeft => (
                        &entry.right_digest,
                        &entry.left_digest,
                        &entry.right_link,
                        &entry.left_link,
                    ),
                };
            if action.source.is_some() {
                check_parents(vfs, source_root, &action.relative_path)?;
                require_reviewed(
                    vfs,
                    &source,
                    action.source.as_ref(),
                    source_digest.as_deref(),
                    source_link.as_deref(),
                    cancel,
                )?;
            }
            require_reviewed(
                vfs,
                &destination,
                action.destination.as_ref(),
                destination_digest.as_deref(),
                destination_link.as_deref(),
                cancel,
            )?;
            match action.kind {
                SyncActionKind::CreateDirectory => vfs
                    .create_dir(&destination)
                    .map_err(|error| error.to_string()),
                SyncActionKind::Copy | SyncActionKind::Replace => {
                    let job = engine.spawn_copy(
                        vec![source],
                        destination.parent().ok_or("Destination has no parent")?,
                        ScanOptions::default(),
                        TransferOptions {
                            conflict_policy: ConflictPolicy::Ask,
                            verify: true,
                            durable: true,
                            parallel: false,
                        },
                    );
                    await_sync_job(
                        vfs,
                        job,
                        cancel,
                        Some((
                            destination,
                            action.destination.as_ref(),
                            destination_digest.as_deref(),
                            destination_link.as_deref(),
                        )),
                    )
                }
                SyncActionKind::Trash => {
                    if action
                        .destination
                        .as_ref()
                        .is_some_and(|meta| meta.kind == EntryKind::Directory)
                    {
                        let descendants = collect_tree_checked(vfs, &destination, cancel, true)?;
                        let expected: Vec<_> = plan
                            .entries
                            .iter()
                            .filter(|entry| {
                                entry.relative_path != action.relative_path
                                    && entry.relative_path.starts_with(&action.relative_path)
                            })
                            .collect();
                        if descendants.len() != expected.len() {
                            return Err(format!(
                                "{destination} changed since review. Compare again."
                            ));
                        }
                        for entry in expected {
                            let relative = entry
                                .relative_path
                                .strip_prefix(&action.relative_path)
                                .map_err(|error| error.to_string())?;
                            let (metadata, digest, link) = match plan.direction {
                                SyncDirection::LeftToRight => {
                                    (&entry.right, &entry.right_digest, &entry.right_link)
                                }
                                SyncDirection::RightToLeft => {
                                    (&entry.left, &entry.left_digest, &entry.left_link)
                                }
                            };
                            if !same_metadata(descendants.get(relative), metadata.as_ref()) {
                                return Err(format!(
                                    "{destination} changed since review. Compare again."
                                ));
                            }
                            require_reviewed(
                                vfs,
                                &VPath::from(destination.as_path().join(relative)),
                                metadata.as_ref(),
                                digest.as_deref(),
                                link.as_deref(),
                                cancel,
                            )?;
                        }
                    }
                    await_sync_job(vfs, engine.spawn_trash(vec![destination]), cancel, None)
                }
                SyncActionKind::Blocked => unreachable!("blocked plans were rejected"),
            }
        })();
        result.map_err(|error| {
            format!(
                "Stopped after {completed} of {} changes: {error}",
                plan.actions.len()
            )
        })?;
        completed += 1;
    }
    Ok(completed)
}

type ReviewedTarget<'a> = (
    VPath,
    Option<&'a Metadata>,
    Option<&'a str>,
    Option<&'a Path>,
);

fn require_reviewed(
    vfs: &dyn Vfs,
    path: &VPath,
    expected: Option<&Metadata>,
    digest: Option<&str>,
    link: Option<&Path>,
    cancel: &CancelToken,
) -> Result<(), String> {
    check_cancelled(cancel)?;
    let current = match vfs.stat(path, false) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => None,
        Err(error) => return Err(error.to_string()),
    };
    if !same_metadata(current.as_ref(), expected)
        || digest
            .is_some_and(|expected| super::sha256(vfs, path, cancel).as_deref() != Ok(expected))
        || link.is_some_and(|expected| !vfs.read_link(path).is_ok_and(|link| link == expected))
    {
        return Err(format!(
            "{path} changed or became unreadable since review. Compare again."
        ));
    }
    Ok(())
}

fn check_parents(vfs: &dyn Vfs, root: &VPath, relative: &Path) -> Result<(), String> {
    let mut path = root.as_path().to_path_buf();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            path.push(component);
            if vfs
                .stat(&VPath::from(path.as_path()), false)
                .map_err(|error| error.to_string())?
                .kind
                != EntryKind::Directory
            {
                return Err(format!(
                    "{} is no longer a directory. Compare again.",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn await_sync_job(
    vfs: &dyn Vfs,
    job: JobHandle,
    cancel: &CancelToken,
    expected: Option<ReviewedTarget<'_>>,
) -> Result<(), String> {
    loop {
        if cancel.is_cancelled() {
            job.cancel();
        }
        match job.events().recv_timeout(Duration::from_millis(40)) {
            Ok(JobEvent::Finished { .. }) => break,
            Ok(JobEvent::Conflict {
                conflict_id,
                conflict,
                ..
            }) => {
                let acceptable = expected
                    .as_ref()
                    .is_some_and(|(path, metadata, digest, link)| {
                        conflict.destination == *path
                            && metadata.is_some()
                            && require_reviewed(vfs, path, *metadata, *digest, *link, cancel)
                                .is_ok()
                    });
                if !acceptable {
                    job.cancel();
                } else {
                    let _ = job.resolve_conflict(
                        conflict_id,
                        ConflictDecision {
                            action: ConflictAction::Overwrite,
                            apply_to_all: false,
                        },
                    );
                }
            }
            Ok(_) => {}
            Err(error) if error.is_timeout() => {}
            Err(_) => break,
        }
    }
    let summary = job.join();
    if summary.state != JobState::Done || !summary.outcome.errors.is_empty() {
        return Err(summary.outcome.errors.first().map_or_else(
            || format!("Transfer ended in {:?}", summary.state),
            |error| error.message.clone(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
