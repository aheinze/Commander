//! Durable, append-only operation records. Intents are synced before filesystem mutations.
//! A truncated final line is recoverable; earlier records are never guessed or discarded.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    JobError, JobErrorKind, JobKind, JobState, TransferOutcome, TransferRecord, TrashRecord,
};
use dualpane_core::{Metadata, VPath};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupRecord {
    pub original: VPath,
    pub backup: VPath,
    pub metadata: Metadata,
}

/// A byte-verified archive replacement and the durable original needed to reverse it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchiveChange {
    pub source: VPath,
    pub backup: VPath,
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MutationIntent {
    pub sequence: u64,
    pub action: String,
    pub source: Option<VPath>,
    pub target: VPath,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum JournalEvent {
    ArchivePrepared {
        change: ArchiveChange,
    },
    Started {
        version: u32,
        kind: JobKind,
        sources: Vec<VPath>,
        destination: Option<VPath>,
    },
    Intent {
        intent: MutationIntent,
    },
    Applied {
        sequence: u64,
    },
    Backup {
        record: BackupRecord,
    },
    Transfer {
        record: TransferRecord,
    },
    Trash {
        record: TrashRecord,
    },
    Finished {
        state: JobState,
        errors: Vec<String>,
        completed_items: u64,
    },
    Reviewed,
}

#[derive(Debug)]
struct Writer {
    file: File,
    sequence: u64,
    transfers: BTreeMap<(VPath, VPath), TransferRecord>,
    trash: Vec<TrashRecord>,
    backups: Vec<BackupRecord>,
    intents: BTreeMap<u64, MutationIntent>,
    deleted_items: u64,
}

#[derive(Debug)]
pub struct JobJournal {
    path: PathBuf,
    writer: Mutex<Writer>,
}

impl Drop for JobJournal {
    fn drop(&mut self) {
        // Closing our descriptor alone can leave the lock held by a child
        // between fork and exec. Release it when the last journal owner exits.
        let writer = self
            .writer
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        let _ = writer.file.unlock();
    }
}

impl JobJournal {
    pub fn create(
        directory: &Path,
        kind: JobKind,
        sources: Vec<VPath>,
        destination: Option<VPath>,
    ) -> Result<Self, JobError> {
        let create = || -> std::io::Result<(PathBuf, File)> {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)?;
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = directory.join(format!("{nonce:032x}-{}.jsonl", std::process::id()));
            let file = OpenOptions::new()
                .create_new(true)
                .read(true)
                .append(true)
                .mode(0o600)
                .open(&path)?;
            file.try_lock().map_err(std::io::Error::from)?;
            File::open(directory)?.sync_all()?;
            Ok((path, file))
        };
        let (path, file) = create().map_err(|error| journal_error(directory, error))?;
        let journal = Self {
            path,
            writer: Mutex::new(Writer {
                file,
                sequence: 0,
                transfers: BTreeMap::new(),
                trash: Vec::new(),
                backups: Vec::new(),
                intents: BTreeMap::new(),
                deleted_items: 0,
            }),
        };
        journal.append(&JournalEvent::Started {
            version: 1,
            kind,
            sources,
            destination,
        })?;
        Ok(journal)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &JournalEvent) -> Result<(), JobError> {
        let mut writer = self.writer.lock().expect("journal mutex poisoned");
        write_event(&mut writer.file, event).map_err(|error| journal_error(&self.path, error))
    }

    pub fn intent(
        &self,
        action: &str,
        source: Option<&VPath>,
        target: &VPath,
    ) -> Result<u64, JobError> {
        let mut writer = self.writer.lock().expect("journal mutex poisoned");
        writer.sequence += 1;
        let sequence = writer.sequence;
        write_event(
            &mut writer.file,
            &JournalEvent::Intent {
                intent: MutationIntent {
                    sequence,
                    action: action.to_owned(),
                    source: source.cloned(),
                    target: target.clone(),
                },
            },
        )
        .map_err(|error| journal_error(&self.path, error))?;
        writer.intents.insert(
            sequence,
            MutationIntent {
                sequence,
                action: action.to_owned(),
                source: source.cloned(),
                target: target.clone(),
            },
        );
        Ok(sequence)
    }

    pub fn applied(&self, sequence: u64) -> Result<(), JobError> {
        let mut writer = self.writer.lock().expect("journal mutex poisoned");
        write_event(&mut writer.file, &JournalEvent::Applied { sequence })
            .map_err(|error| journal_error(&self.path, error))?;
        if writer
            .intents
            .remove(&sequence)
            .is_some_and(|intent| intent.action == "delete permanently")
        {
            writer.deleted_items += 1;
        }
        Ok(())
    }

    /// Persist the backup address *before* moving the original to it.
    pub fn backup(&self, record: BackupRecord) -> Result<(), JobError> {
        let mut writer = self.writer.lock().expect("journal mutex poisoned");
        write_event(
            &mut writer.file,
            &JournalEvent::Backup {
                record: record.clone(),
            },
        )
        .map_err(|error| journal_error(&self.path, error))?;
        writer.backups.push(record);
        Ok(())
    }

    pub fn transfer(&self, record: &TransferRecord) -> Result<(), JobError> {
        let mut writer = self.writer.lock().expect("journal mutex poisoned");
        write_event(
            &mut writer.file,
            &JournalEvent::Transfer {
                record: record.clone(),
            },
        )
        .map_err(|error| journal_error(&self.path, error))?;
        writer.transfers.insert(
            (record.source.clone(), record.destination.clone()),
            record.clone(),
        );
        Ok(())
    }

    pub fn trash(&self, record: &TrashRecord) -> Result<(), JobError> {
        let mut writer = self.writer.lock().expect("journal mutex poisoned");
        write_event(
            &mut writer.file,
            &JournalEvent::Trash {
                record: record.clone(),
            },
        )
        .map_err(|error| journal_error(&self.path, error))?;
        writer.trash.push(record.clone());
        Ok(())
    }

    pub fn partial_outcome(&self) -> TransferOutcome {
        let writer = self.writer.lock().expect("journal mutex poisoned");
        TransferOutcome {
            completed_items: (writer.transfers.len() + writer.trash.len()) as u64
                + writer.deleted_items,
            transfers: writer.transfers.values().cloned().collect(),
            trash_records: writer.trash.clone(),
            backups: writer.backups.clone(),
            journal_path: Some(self.path.clone()),
            ..TransferOutcome::default()
        }
    }
}

fn write_event(file: &mut File, event: &JournalEvent) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(event)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_all()
}

fn journal_error(path: &Path, error: std::io::Error) -> JobError {
    JobError {
        path: VPath::from(path),
        operation: "persist operation journal",
        kind: JobErrorKind::IoError,
        message: format!(
            "Could not persist the operation journal at {}: {error}",
            path.display()
        ),
    }
}

#[derive(Clone, Debug)]
pub struct RecoveryRecord {
    pub path: PathBuf,
    pub kind: JobKind,
    pub sources: Vec<VPath>,
    pub destination: Option<VPath>,
    pub state: Option<JobState>,
    pub completed_items: u64,
    pub transfers: Vec<TransferRecord>,
    pub trash: Vec<TrashRecord>,
    pub backups: Vec<BackupRecord>,
    pub pending: Vec<MutationIntent>,
    pub applied: Vec<MutationIntent>,
    pub errors: Vec<String>,
    pub reviewed: bool,
    pub incomplete_tail: bool,
    pub archive: Option<ArchiveChange>,
}

pub fn read_journal(path: &Path) -> std::io::Result<RecoveryRecord> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    // A different live Commander still owns this job; never offer recovery for it.
    file.try_lock().map_err(std::io::Error::from)?;
    let mut record: Option<RecoveryRecord> = None;
    let mut pending = BTreeMap::new();
    let mut transfers = BTreeMap::new();
    for line in BufReader::new(&file).split(b'\n') {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let event: JournalEvent = match serde_json::from_slice(&line) {
            Ok(event) => event,
            Err(error) => {
                if let Some(record) = record.as_mut() {
                    record.incomplete_tail = true;
                    record.errors.push(format!(
                        "Journal has an incomplete or damaged record: {error}"
                    ));
                    break;
                }
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error));
            }
        };
        if let JournalEvent::Started {
            version,
            kind,
            sources,
            destination,
        } = event
        {
            if record.is_some() || version != 1 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unsupported journal format",
                ));
            }
            record = Some(RecoveryRecord {
                path: path.to_owned(),
                kind,
                sources,
                destination,
                state: None,
                completed_items: 0,
                transfers: Vec::new(),
                trash: Vec::new(),
                backups: Vec::new(),
                pending: Vec::new(),
                applied: Vec::new(),
                errors: Vec::new(),
                reviewed: false,
                incomplete_tail: false,
                archive: None,
            });
            continue;
        }
        let Some(record) = record.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "missing journal header",
            ));
        };
        match event {
            JournalEvent::ArchivePrepared { change } => record.archive = Some(change),
            JournalEvent::Intent { intent } => {
                pending.insert(intent.sequence, intent);
            }
            JournalEvent::Applied { sequence } => {
                if let Some(intent) = pending.remove(&sequence) {
                    record.applied.push(intent);
                }
            }
            JournalEvent::Backup { record: backup } => record.backups.push(backup),
            JournalEvent::Transfer { record: transfer } => {
                transfers.insert(
                    (transfer.source.clone(), transfer.destination.clone()),
                    transfer,
                );
            }
            JournalEvent::Trash { record: trash } => record.trash.push(trash),
            JournalEvent::Finished {
                state,
                errors,
                completed_items,
            } => {
                record.state = Some(state);
                record.errors.extend(errors);
                record.completed_items = completed_items;
            }
            JournalEvent::Reviewed => record.reviewed = true,
            JournalEvent::Started { .. } => unreachable!(),
        }
    }
    let mut record = record
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "empty journal"))?;
    record.transfers = transfers.into_values().collect();
    record.completed_items = record
        .completed_items
        .max((record.transfers.len() + record.trash.len()) as u64);
    record.pending = pending.into_values().collect();
    record.reviewed |= path.with_extension("reviewed").is_file();
    Ok(record)
}

pub fn scan_journals(directory: &Path) -> (Vec<RecoveryRecord>, Vec<String>) {
    let mut records = Vec::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (records, errors),
        Err(error) => return (records, vec![error.to_string()]),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        };
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
            || !entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            continue;
        }
        match read_journal(&path) {
            Ok(mut record) => {
                // The centre renders summaries. Restore explicitly reloads the full
                // record, so a long operation history cannot fill UI memory.
                record.transfers.truncate(100);
                record.backups.truncate(100);
                record.trash.truncate(100);
                record.pending.truncate(100);
                record.applied.truncate(100);
                records.push(record);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    records.sort_by(|a, b| a.path.cmp(&b.path));
    (records, errors)
}

pub fn mark_reviewed(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    file.try_lock().map_err(std::io::Error::from)?;
    let marker = path.with_extension("reviewed");
    match OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&marker)
    {
        Ok(marker) => marker.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finished_journal_releases_lock_while_duplicated_descriptor_exists() {
        let directory = tempfile::tempdir().unwrap();
        let journal =
            JobJournal::create(directory.path(), JobKind::Copy, Vec::new(), None).unwrap();
        journal
            .append(&JournalEvent::Finished {
                state: JobState::Done,
                errors: Vec::new(),
                completed_items: 0,
            })
            .unwrap();
        let path = journal.path().to_owned();
        // A concurrent fork briefly inherits the same open file description
        // before exec closes CLOEXEC descriptors. A clone models that lifetime.
        let inherited = journal.writer.lock().unwrap().file.try_clone().unwrap();
        assert_eq!(
            read_journal(&path).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(journal);
        assert_eq!(read_journal(&path).unwrap().state, Some(JobState::Done));
        drop(inherited);
    }

    #[test]
    fn active_journals_are_excluded_and_truncated_tails_keep_the_intent() {
        let directory = tempfile::tempdir().unwrap();
        let journal = JobJournal::create(
            directory.path(),
            JobKind::Copy,
            vec![VPath::from("/source")],
            Some(VPath::from("/destination")),
        )
        .unwrap();
        journal
            .intent(
                "publish",
                Some(&VPath::from("/temporary")),
                &VPath::from("/destination/file"),
            )
            .unwrap();
        assert!(scan_journals(directory.path()).0.is_empty());
        let path = journal.path().to_owned();
        drop(journal);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"event\":")
            .unwrap();
        let record = read_journal(&path).unwrap();
        assert!(record.incomplete_tail);
        assert_eq!(record.pending.len(), 1);
        assert!(record.state.is_none());
    }
    #[test]
    fn native_non_utf8_paths_round_trip_losslessly() {
        use std::os::unix::ffi::OsStringExt;
        let path = VPath::from(PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/\xff\nfile".to_vec(),
        )));
        let encoded = serde_json::to_vec(&path).unwrap();
        assert_eq!(serde_json::from_slice::<VPath>(&encoded).unwrap(), path);
    }
    #[test]
    fn unavailable_journal_prevents_a_transfer_before_mutation() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        fs::write(&source, b"source").unwrap();
        fs::create_dir(&destination).unwrap();
        let blocker = fixture.path().join("not-a-directory");
        fs::write(&blocker, b"keep").unwrap();
        let result = crate::OperationEngine::default()
            .with_journal_directory(blocker)
            .spawn_copy(
                vec![VPath::from(source.as_path())],
                VPath::from(destination.as_path()),
                crate::ScanOptions::default(),
                crate::TransferOptions::default(),
            )
            .join();
        assert_eq!(result.state, JobState::Failed);
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }
}
