//! Reviewed retries from durable operation records. Never replay raw mutation intents.
use crate::{
    JobError, JobErrorKind, JobKind, JobState,
    journal::{self, JobJournal, RecoveryRecord},
};
use dualpane_core::VPath;
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
};

#[derive(Clone, Debug)]
pub struct RecoveryPlan {
    record: RecoveryRecord,
}

impl RecoveryPlan {
    /// Read the complete journal, not the abbreviated Recovery list entry.
    pub fn load(path: &Path) -> io::Result<Self> {
        let record = journal::read_journal(path)?;
        eligible(&record)?;
        Ok(Self { record })
    }
    pub fn record(&self) -> &RecoveryRecord {
        &self.record
    }

    /// Re-read under the journal lock and reject a stale review before any job starts.
    pub fn claim(&self) -> io::Result<RecoveryClaim> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.record.path)?;
        lock.try_lock().map_err(io::Error::from)?;
        let current = journal::read_locked_journal(&self.record.path, &lock)?;
        eligible(&current)?;
        if current != self.record {
            return Err(io::Error::other(
                "This operation record changed. Open Recovery and review it again.",
            ));
        }
        Ok(RecoveryClaim {
            record: current,
            _lock: lock,
        })
    }
}

pub fn can_retry(record: &RecoveryRecord) -> bool {
    eligible(record).is_ok()
}

fn eligible(record: &RecoveryRecord) -> io::Result<()> {
    if !matches!(record.kind, JobKind::Copy | JobKind::Move)
        || record.archive.is_some()
        || record
            .pending
            .iter()
            .chain(&record.applied)
            .any(|step| step.action == "update archive")
    {
        return Err(io::Error::other(
            "Only interrupted copy and move operations can be retried here.",
        ));
    }
    if !matches!(
        record.state,
        None | Some(JobState::Failed | JobState::Cancelled)
    ) {
        return Err(io::Error::other(
            "This operation is already complete or still running.",
        ));
    }
    if record.retry.is_some() {
        return Err(io::Error::other(
            "This operation has already been retried. Review its newer attempt in Recovery.",
        ));
    }
    if record.sources.is_empty() || record.destination.is_none() {
        return Err(io::Error::other(
            "The operation record has no sources or destination to retry.",
        ));
    }
    if record
        .sources
        .iter()
        .any(|source| !source.as_path().is_absolute())
        || !record.destination.as_ref().unwrap().as_path().is_absolute()
    {
        return Err(io::Error::other(
            "This operation uses relative paths and cannot be safely retried after a restart.",
        ));
    }
    Ok(())
}

/// Exclusive ownership of the original journal until the new attempt finishes.
#[derive(Debug)]
pub struct RecoveryClaim {
    record: RecoveryRecord,
    _lock: File,
}
impl RecoveryClaim {
    pub fn record(&self) -> &RecoveryRecord {
        &self.record
    }
    pub(crate) fn link(&self, next: &JobJournal) -> Result<(), JobError> {
        let path = self.record.path.with_extension("retry");
        // Publish a fully synced marker without overwriting an existing successor.
        let temporary = next.path().with_extension("retry-link");
        let persist = || -> io::Result<()> {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            let contents = serde_json::to_vec(next.path()).map_err(io::Error::other)?;
            file.write_all(&contents)?;
            file.sync_all()?;
            std::fs::hard_link(&temporary, &path)?;
            std::fs::remove_file(&temporary)?;
            if let Some(parent) = path.parent() {
                File::open(parent)?.sync_all()?;
            }
            Ok(())
        };
        persist().map_err(|error| JobError {
            path: VPath::from(path),
            operation: "record recovery retry",
            kind: JobErrorKind::IoError,
            message: format!("Could not link the retry to its original operation: {error}"),
        })
    }
}

impl Drop for RecoveryClaim {
    fn drop(&mut self) {
        let _ = self._lock.unlock();
    }
}
