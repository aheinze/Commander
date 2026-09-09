//! Shared publication path for edits, undo/redo, and recovery.
use super::*;

pub(super) fn publish(
    snapshot: &Snapshot,
    output: tempfile::NamedTempFile,
    task: &mut ArchiveTask<'_>,
) -> Result<ArchiveChange, String> {
    let cancel = task.cancel.clone();
    let cancel = &cancel;
    let parent = snapshot
        .source
        .as_path()
        .parent()
        .ok_or("Archive has no parent folder")?;
    task.finishing()?;
    task.check()
        .map_err(|_| "Archive save cancelled".to_owned())?;
    let permissions = writable_permissions(&snapshot.source)?;
    output
        .as_file()
        .set_permissions(permissions)
        .map_err(|e| e.to_string())?;
    output.as_file().sync_all().map_err(|e| e.to_string())?;
    // A durable recovery copy also detects a file modified during compression.
    let mut backup = tempfile::Builder::new()
        .prefix(".commander-archive-backup-")
        .suffix(&format!(".{}", snapshot.format.extension()))
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    let input = File::open(snapshot.source.as_path()).map_err(|e| e.to_string())?;
    io::copy(
        &mut Cancellable {
            inner: input,
            cancel,
            control: task.control(),
        },
        &mut backup,
    )
    .map_err(|e| e.to_string())?;
    backup.as_file().sync_all().map_err(|e| e.to_string())?;
    if crate::features::sha256(&LocalFs, &VPath::from(backup.path()), cancel)? != snapshot.digest
        || crate::features::sha256(&LocalFs, &snapshot.source, cancel)? != snapshot.digest
    {
        return Err("Archive changed during save. Original archive was not replaced.".into());
    }
    task.check()
        .map_err(|_| "Archive save cancelled".to_owned())?;
    writable_permissions(&snapshot.source)?;
    let (_, backup_path) = backup.keep().map_err(|e| e.to_string())?;
    let change = ArchiveChange {
        source: snapshot.source.clone(),
        backup: VPath::from(backup_path.as_path()),
        before: snapshot.digest.clone(),
        after: crate::features::sha256(&LocalFs, &VPath::from(output.path()), cancel)?,
    };
    let journal = task
        .journal()
        .map(|journal| {
            use dualpane_engine::journal::{BackupRecord, JournalEvent};
            let sequence = journal
                .intent("update archive", Some(&change.backup), &change.source)
                .map_err(|e| e.message)?;
            let metadata = LocalFs
                .stat(&change.backup, false)
                .map_err(|e| e.to_string())?;
            journal
                .backup(BackupRecord {
                    original: change.source.clone(),
                    backup: change.backup.clone(),
                    metadata,
                })
                .map_err(|e| e.message)?;
            journal
                .append(&JournalEvent::ArchivePrepared {
                    change: change.clone(),
                })
                .map_err(|e| e.message)?;
            Ok::<_, String>((journal, sequence))
        })
        .transpose()?;
    // Hashing and journaling can take time: check the current source again before publication.
    if crate::features::sha256(&LocalFs, &snapshot.source, cancel)? != snapshot.digest {
        return Err(format!(
            "Archive changed before publication. Recovery copy: {}",
            backup_path.display()
        ));
    }
    task.check()
        .map_err(|_| "Archive update cancelled".to_owned())?;
    output.persist(snapshot.source.as_path()).map_err(|e| {
        format!(
            "Could not publish archive: {e}. Recovery copy: {}",
            backup_path.display()
        )
    })?;
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|e| {
            format!(
                "Archive saved, but folder sync failed: {e}. Recovery copy: {}",
                backup_path.display()
            )
        })?;
    if let Some((journal, sequence)) = journal {
        journal.applied(sequence).map_err(|e| e.message)?;
    }
    Ok(change)
}
/// Restoring creates a replacement record that can itself be reversed (redo).
pub fn restore(
    change: &ArchiveChange,
    task: &mut ArchiveTask<'_>,
) -> Result<ArchiveChange, String> {
    writable_permissions(&change.source)?;
    if !fs::symlink_metadata(change.backup.as_path())
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("The archive recovery copy is not a regular file.".into());
    }
    if crate::features::sha256(&LocalFs, &change.source, &task.cancel)? != change.after {
        return Err("The archive changed after this operation. It was not replaced. Inspect the recovery copy before restoring it.".into());
    }
    if crate::features::sha256(&LocalFs, &change.backup, &task.cancel)? != change.before {
        return Err("The archive recovery copy changed. Nothing was restored.".into());
    }
    let parent = change
        .source
        .as_path()
        .parent()
        .ok_or("Archive has no parent folder")?;
    let mut output = tempfile::Builder::new()
        .prefix(".commander-archive-restore-")
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    task.begin(&change.source);
    copy_reader(
        File::open(change.backup.as_path()).map_err(|e| e.to_string())?,
        &mut output,
        task,
    )?;
    if crate::features::sha256(&LocalFs, &VPath::from(output.path()), &task.cancel)?
        != change.before
    {
        return Err("The recovery copy changed while reading it. Nothing was restored.".into());
    }
    publish(
        &Snapshot {
            source: change.source.clone(),
            digest: change.after.clone(),
            format: format_for(&change.source)?,
            items: Vec::new(),
            password: None,
        },
        output,
        task,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualpane_engine::journal::{JobJournal, JournalEvent, read_journal};
    use dualpane_engine::{ConflictPolicy, JobKind, JobState};
    use std::sync::Arc;

    #[test]
    fn archive_edits_undo_redo_and_journals_preserve_both_versions_in_every_format() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("notes.txt");
        fs::write(&file, "original").unwrap();
        let cancel = CancelToken::new();
        for format in [
            ArchiveFormat::Zip,
            ArchiveFormat::SevenZ,
            ArchiveFormat::Tar,
            ArchiveFormat::TarGz,
        ] {
            let source = VPath::from(fixture.path().join(format!("test.{}", format.extension())));
            create_archive(
                &LocalFs,
                &[VPath::from(file.as_path())],
                &source,
                format,
                &mut ArchiveTask::new(&cancel),
            )
            .unwrap();
            let original = fs::read(source.as_path()).unwrap();
            let snapshot = inspect(&source, &cancel).unwrap();
            let root = tempfile::tempdir().unwrap();
            extract_archive(
                &LocalFs,
                &source,
                &VPath::from(root.path()),
                &mut ArchiveTask::new(&cancel),
            )
            .unwrap();
            let journal = Arc::new(
                JobJournal::create(
                    &fixture.path().join("jobs"),
                    JobKind::Copy,
                    vec![source.clone()],
                    Some(source.clone()),
                )
                .unwrap(),
            );
            let journal_path = journal.path().to_owned();
            let mut task = ArchiveTask::new(&cancel);
            task.set_journal(journal.clone());
            let change = apply(
                &snapshot,
                root.path(),
                &Action::Create {
                    path: "new folder".into(),
                    directory: true,
                },
                &mut task,
                |_| Ok(ConflictPolicy::Skip),
            )
            .unwrap()
            .unwrap();
            let updated = fs::read(source.as_path()).unwrap();
            assert_ne!(updated, original);
            journal
                .append(&JournalEvent::Finished {
                    state: JobState::Done,
                    errors: Vec::new(),
                    completed_items: 1,
                })
                .unwrap();
            drop(task);
            drop(journal);
            let saved = read_journal(&journal_path).unwrap();
            assert_eq!(saved.archive.as_ref(), Some(&change));
            assert_eq!(saved.backups.len(), 1);
            assert!(saved.pending.is_empty());
            assert_eq!(saved.applied[0].action, "update archive");
            let undo = restore(&change, &mut ArchiveTask::new(&cancel)).unwrap();
            assert_eq!(fs::read(source.as_path()).unwrap(), original);
            assert_eq!(fs::read(undo.backup.as_path()).unwrap(), updated);
            let redo = restore(&undo, &mut ArchiveTask::new(&cancel)).unwrap();
            assert_eq!(fs::read(source.as_path()).unwrap(), updated);
            assert_eq!(fs::read(redo.backup.as_path()).unwrap(), original);
            // A later external edit must never be silently overwritten by undo/recovery.
            fs::write(source.as_path(), "external change").unwrap();
            assert!(
                restore(&redo, &mut ArchiveTask::new(&cancel))
                    .unwrap_err()
                    .contains("changed after")
            );
            assert_eq!(fs::read(source.as_path()).unwrap(), b"external change");
            fs::write(source.as_path(), &updated).unwrap();
            fs::write(redo.backup.as_path(), "damaged backup").unwrap();
            assert!(
                restore(&redo, &mut ArchiveTask::new(&cancel))
                    .unwrap_err()
                    .contains("recovery copy changed")
            );
            assert_eq!(fs::read(source.as_path()).unwrap(), updated);
        }
    }
}
