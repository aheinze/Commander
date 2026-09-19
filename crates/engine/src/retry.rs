//! Reuse only committed destinations from the previous attempt. Never trust existence alone.
use crate::{JobControl, PlanEntry, TransferRecord};
use dualpane_core::{EntryKind, VPath};
use dualpane_vfs::Vfs;

pub(crate) fn unchanged_destination(vfs: &dyn Vfs, record: &TransferRecord) -> bool {
    vfs.stat(&record.destination, false).is_ok_and(|current| {
        current.kind == record.metadata.kind
            && current.identity == record.metadata.identity
            && (current.kind == EntryKind::Directory
                || (current.size == record.metadata.size
                    && current.modified == record.metadata.modified))
    })
}

pub(crate) fn remaining_move_sources(
    vfs: &dyn Vfs,
    sources: Vec<VPath>,
    control: &JobControl,
) -> Vec<VPath> {
    sources
        .into_iter()
        .filter(|source| {
            !control.retry_record(source).is_some_and(|record| {
                record.source_removed
                    && vfs
                        .stat(source, false)
                        .is_err_and(|error| error.io_kind() == Some(std::io::ErrorKind::NotFound))
                    && (unchanged_destination(vfs, record)
                        || verified_removed_destination(vfs, record, control))
            })
        })
        .collect()
}

/// A previous Keep Both destination is usable only within the currently selected parent.
pub(crate) fn candidate<'a>(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    target: &VPath,
    control: &'a JobControl,
) -> Option<&'a TransferRecord> {
    control.retry_record(&entry.source).filter(|record| {
        record.destination.parent() == target.parent()
            && record.metadata.kind == entry.metadata.kind
            && unchanged_destination(vfs, record)
    })
}

pub(crate) fn verified(
    vfs: &dyn Vfs,
    entry: &PlanEntry,
    record: &TransferRecord,
    control: &JobControl,
) -> bool {
    let valid = match entry.metadata.kind {
        EntryKind::File => {
            crate::verify_copy_controlled(vfs, &entry.source, &record.destination, control).is_ok()
        }
        EntryKind::Symlink => {
            matches!((vfs.read_link(&entry.source), vfs.read_link(&record.destination)), (Ok(left), Ok(right)) if left == right)
        }
        _ => false,
    };
    valid && unchanged_destination(vfs, record)
}

/// A disconnected GVfs mount can return with different synthetic inode IDs.
/// Accept that change only with saved byte proof, never size/mtime alone.
fn verified_removed_destination(
    vfs: &dyn Vfs,
    record: &TransferRecord,
    control: &JobControl,
) -> bool {
    if record.metadata.kind != EntryKind::Directory {
        return fingerprint_matches(vfs, record, control);
    }
    let Ok(plan) = crate::scan_sources(
        vfs,
        std::slice::from_ref(&record.destination),
        crate::ScanOptions::default(),
        control,
        |_| {},
    ) else {
        return false;
    };
    if !plan.errors.is_empty() {
        return false;
    }
    let expected: std::collections::BTreeMap<_, _> = control
        .retry_records()
        .filter(|item| {
            item.source_removed
                && item.source.as_path().starts_with(record.source.as_path())
                && item
                    .destination
                    .as_path()
                    .starts_with(record.destination.as_path())
        })
        .map(|item| (&item.destination, item))
        .collect();
    // Extra/missing children make a previously moved directory a changed output.
    plan.entries.len() == expected.len()
        && plan.entries.iter().all(|entry| {
            expected.get(&entry.source).is_some_and(|item| {
                if item.metadata.kind == EntryKind::Directory {
                    entry.metadata.kind == EntryKind::Directory
                } else {
                    fingerprint_matches(vfs, item, control)
                }
            })
        })
}

fn fingerprint_matches(vfs: &dyn Vfs, record: &TransferRecord, control: &JobControl) -> bool {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let Some(expected) = &record.fingerprint else {
        return false;
    };
    let Ok(before) = vfs.stat(&record.destination, false) else {
        return false;
    };
    if before.kind != record.metadata.kind
        || before.size != record.metadata.size
        || before.modified != record.metadata.modified
    {
        return false;
    }
    let mut digest = Sha256::new();
    match before.kind {
        EntryKind::File => {
            let Ok(mut file) = vfs.open_read(&record.destination) else {
                return false;
            };
            let mut buffer = [0; 64 * 1024];
            let mut remaining = before.size;
            while remaining > 0 {
                if control.checkpoint().is_err() {
                    return false;
                }
                control.phase(crate::JobPhase::Verifying, Some(record.destination.clone()));
                let count = remaining.min(buffer.len() as u64) as usize;
                if file.read_exact(&mut buffer[..count]).is_err() {
                    return false;
                }
                digest.update(&buffer[..count]);
                remaining -= count as u64;
            }
        }
        EntryKind::Symlink => {
            let Ok(target) = vfs.read_link(&record.destination) else {
                return false;
            };
            digest.update(target.as_os_str().as_encoded_bytes());
        }
        _ => return false,
    }
    format!("{:x}", digest.finalize()) == *expected
        && vfs.stat(&record.destination, false).is_ok_and(|after| {
            after.kind == before.kind
                && after.identity == before.identity
                && after.size == before.size
                && after.modified == before.modified
        })
}
