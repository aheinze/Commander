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
                    && unchanged_destination(vfs, record)
                    && vfs
                        .stat(source, false)
                        .is_err_and(|error| error.io_kind() == Some(std::io::ErrorKind::NotFound))
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
