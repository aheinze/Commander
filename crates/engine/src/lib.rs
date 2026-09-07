#![forbid(unsafe_code)]

//! Cancellable scanning and file-operation jobs.

mod conflict;
mod error;
mod job;
pub mod journal;
mod operations;
mod runner;
mod scan;
mod transfer;

pub use conflict::{
    Conflict, ConflictAction, ConflictDecision, ConflictPolicy, ConflictResolution,
    resolve_conflict, unique_renamed_path,
};
pub use dualpane_core::{CancelToken, Cancelled};
pub use error::{JobError, JobErrorKind};
pub use job::{
    ConflictId, ConflictResponse, JobControl, JobEvent, JobId, JobKind, JobPhase, JobProgress,
    JobState, JobSummary, ProgressEmitter,
};
pub use operations::{delete_permanently, move_paths, move_sources, trash_sources};
pub use runner::{JobHandle, OperationEngine};
pub use scan::{PlanEntry, ScanOptions, ScanPlan, ScanProgress, scan_sources};
pub use transfer::{
    CopyMethod, TransferOptions, TransferOutcome, TransferProgress, TransferRecord, TrashRecord,
    copy_plan, remove_tree, verify_copy, verify_copy_controlled,
};
