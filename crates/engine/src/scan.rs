use std::collections::{HashMap, HashSet, hash_map};
use std::path::PathBuf;

use dualpane_core::{EntryKind, FileIdentity, Metadata, VPath};
use dualpane_vfs::{Vfs, VfsError};

use crate::{JobControl, JobError};

/// Scan behavior shared by copy and move jobs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanOptions {
    pub dereference_symlinks: bool,
}

/// One transfer item in directory-first order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanEntry {
    pub source: VPath,
    pub relative_path: PathBuf,
    pub metadata: Metadata,
    /// Earlier plan entry whose destination should be hard-linked for this item.
    pub hardlink_to: Option<usize>,
}

/// Complete immutable transfer plan.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanPlan {
    pub entries: Vec<PlanEntry>,
    pub total_bytes: u64,
    pub files: u64,
    pub directories: u64,
    pub errors: Vec<JobError>,
}

/// Incremental scan count suitable for a “Scanning…” status.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanProgress {
    pub items: u64,
    pub bytes: u64,
}

struct Pending {
    source: VPath,
    relative_path: PathBuf,
    ancestors: HashSet<FileIdentity>,
}

/// Walks all sources iteratively and produces a cancellable transfer plan.
///
/// Item errors are retained in the plan so one vanished or unreadable source does not
/// abort the remaining operation.
///
/// # Errors
///
/// Returns cancellation when requested by the job control.
pub fn scan_sources(
    vfs: &dyn Vfs,
    sources: &[VPath],
    options: ScanOptions,
    control: &JobControl,
    mut progress: impl FnMut(ScanProgress),
) -> Result<ScanPlan, dualpane_core::Cancelled> {
    let span = tracing::info_span!("job.scan", sources = sources.len());
    let _guard = span.enter();
    let mut plan = ScanPlan::default();
    let mut hardlinks = HashMap::<FileIdentity, usize>::new();
    let mut pending = Vec::new();
    for source in sources.iter().rev() {
        let relative_path = source
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("root"));
        pending.push(Pending {
            source: source.clone(),
            relative_path,
            ancestors: HashSet::new(),
        });
    }

    while let Some(mut item) = pending.pop() {
        control.checkpoint()?;
        let nofollow = match vfs.stat(&item.source, false) {
            Ok(metadata) => metadata,
            Err(VfsError::Cancelled) => return Err(dualpane_core::Cancelled),
            Err(error) => {
                plan.errors
                    .push(JobError::from_vfs(item.source, "scan metadata", &error));
                continue;
            }
        };
        let metadata = if nofollow.kind == EntryKind::Symlink && options.dereference_symlinks {
            match vfs.stat(&item.source, true) {
                Ok(metadata) => metadata,
                Err(VfsError::Cancelled) => return Err(dualpane_core::Cancelled),
                Err(error) => {
                    plan.errors.push(JobError::from_vfs(
                        item.source,
                        "dereference symlink",
                        &error,
                    ));
                    continue;
                }
            }
        } else {
            nofollow
        };

        if metadata.kind == EntryKind::Directory
            && let Some(identity) = metadata.identity
            && !item.ancestors.insert(identity)
        {
            plan.errors.push(JobError {
                path: item.source,
                operation: "scan directory",
                kind: crate::JobErrorKind::IoError,
                message: "symlink loop detected".to_owned(),
            });
            continue;
        }

        let hardlink_to = if metadata.kind == EntryKind::File
            && metadata.hard_links.is_some_and(|count| count > 1)
        {
            metadata
                .identity
                .and_then(|identity| match hardlinks.entry(identity) {
                    hash_map::Entry::Occupied(first) => Some(*first.get()),
                    hash_map::Entry::Vacant(first) => {
                        first.insert(plan.entries.len());
                        None
                    }
                })
        } else {
            None
        };
        let item_bytes = if metadata.kind == EntryKind::File {
            metadata.size
        } else {
            0
        };
        plan.total_bytes = plan.total_bytes.saturating_add(item_bytes);
        if metadata.kind == EntryKind::Directory {
            plan.directories = plan.directories.saturating_add(1);
        } else {
            plan.files = plan.files.saturating_add(1);
        }
        plan.entries.push(PlanEntry {
            source: item.source.clone(),
            relative_path: item.relative_path.clone(),
            metadata: metadata.clone(),
            hardlink_to,
        });
        progress(ScanProgress {
            items: u64::try_from(plan.entries.len()).unwrap_or(u64::MAX),
            bytes: plan.total_bytes,
        });

        if metadata.kind != EntryKind::Directory {
            continue;
        }
        let directory = match vfs.read_dir(&item.source, &control.cancel_token()) {
            Ok(directory) => directory,
            Err(VfsError::Cancelled) => return Err(dualpane_core::Cancelled),
            Err(error) => {
                plan.errors
                    .push(JobError::from_vfs(item.source, "scan directory", &error));
                continue;
            }
        };
        let mut children = Vec::new();
        for child in directory {
            control.checkpoint()?;
            match child {
                Ok(child) => children.push(Pending {
                    source: item.source.join_name(child.name()),
                    relative_path: item.relative_path.join(child.name()),
                    ancestors: item.ancestors.clone(),
                }),
                Err(VfsError::Cancelled) => return Err(dualpane_core::Cancelled),
                Err(error) => plan.errors.push(JobError::from_vfs(
                    item.source.clone(),
                    "scan directory entry",
                    &error,
                )),
            }
        }
        pending.extend(children.into_iter().rev());
    }

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dualpane_core::VPath;
    use dualpane_vfs::LocalFs;
    use tempfile::tempdir;

    use super::{ScanOptions, scan_sources};
    use crate::JobControl;

    #[test]
    fn scan_is_directory_first_and_counts_bytes() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        fs::create_dir(&source).expect("source");
        fs::create_dir(source.join("nested")).expect("nested");
        fs::write(source.join("nested/file.bin"), b"payload").expect("file");

        let plan = scan_sources(
            &LocalFs,
            &[VPath::from(source.as_path())],
            ScanOptions::default(),
            &JobControl::new(),
            |_| {},
        )
        .expect("scan");

        assert_eq!(plan.entries.len(), 3);
        assert!(plan.entries[0].metadata.kind.is_directory());
        assert!(plan.entries[1].metadata.kind.is_directory());
        assert_eq!(plan.total_bytes, 7);
        assert!(plan.errors.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn scan_records_hardlink_relationships() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        fs::create_dir(&source).expect("source");
        fs::write(source.join("one"), b"payload").expect("file");
        fs::hard_link(source.join("one"), source.join("two")).expect("hardlink");

        let plan = scan_sources(
            &LocalFs,
            &[VPath::from(source.as_path())],
            ScanOptions::default(),
            &JobControl::new(),
            |_| {},
        )
        .expect("scan");
        let linked = plan
            .entries
            .iter()
            .filter(|entry| entry.hardlink_to.is_some())
            .count();

        assert_eq!(linked, 1);
    }

    #[cfg(unix)]
    #[test]
    fn dereferenced_symlink_cycle_is_reported_without_recursing_forever() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        fs::create_dir(&source).expect("source");
        fs::create_dir(source.join("nested")).expect("nested");
        std::os::unix::fs::symlink("..", source.join("nested/back")).expect("cycle");

        let plan = scan_sources(
            &LocalFs,
            &[VPath::from(source.as_path())],
            ScanOptions {
                dereference_symlinks: true,
            },
            &JobControl::new(),
            |_| {},
        )
        .expect("scan");

        assert_eq!(plan.errors.len(), 1);
        assert!(plan.errors[0].message.contains("symlink loop"));
        assert!(plan.entries.len() < 10);
    }
}
