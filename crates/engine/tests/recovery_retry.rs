use dualpane_core::VPath;
use dualpane_engine::{
    journal::{JobJournal, JournalEvent, read_journal, scan_journals},
    recovery::RecoveryPlan,
    *,
};
use dualpane_vfs::{LocalFs, Vfs};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
#[allow(dead_code)]
#[path = "../../vfs/tests/support/fault_fs.rs"]
mod fault_fs;
use fault_fs::{Failure, FaultFs};

fn options() -> TransferOptions {
    TransferOptions {
        conflict_policy: ConflictPolicy::Ask,
        verify: true,
        durable: true,
        parallel: false,
    }
}
fn finish(job: JobHandle) -> JobSummary {
    loop {
        match job
            .events()
            .recv_timeout(Duration::from_secs(10))
            .expect("job stalled")
        {
            JobEvent::Conflict { conflict, .. } => {
                job.cancel();
                panic!("unexpected overwrite: {conflict:?}");
            }
            JobEvent::Finished { .. } => break,
            _ => {}
        }
    }
    job.join()
}
fn journal(
    root: &Path,
    kind: JobKind,
    sources: Vec<VPath>,
    destination: VPath,
    records: &[TransferRecord],
) -> PathBuf {
    let journal =
        JobJournal::create(&root.join("journals"), kind, sources, Some(destination)).unwrap();
    for record in records {
        journal.transfer(record).unwrap();
    }
    journal
        .append(&JournalEvent::Finished {
            state: JobState::Failed,
            errors: vec!["Connection lost".into()],
            completed_items: records.len() as u64,
        })
        .unwrap();
    journal.path().to_owned()
}

#[test]
fn restart_retry_reloads_more_than_the_summary_limit_and_links_the_new_attempt() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let target = root.join("target");
    fs::create_dir(&target).unwrap();
    let sources: Vec<VPath> = (0..121)
        .map(|index| {
            let path = root.join(format!("file-{index:03}"));
            fs::write(&path, format!("payload {index}")).unwrap();
            path.into()
        })
        .collect();
    let original = finish(OperationEngine::default().spawn_copy(
        sources[..120].to_vec(),
        target.clone().into(),
        ScanOptions::default(),
        options(),
    ));
    assert_eq!(original.state, JobState::Done);
    let last = target.join("file-119");
    let identity = LocalFs.stat(&last.clone().into(), false).unwrap().identity;
    let path = journal(
        root,
        JobKind::Copy,
        sources,
        target.clone().into(),
        &original.outcome.transfers,
    );
    let (summaries, errors) = scan_journals(&root.join("journals"));
    assert!(errors.is_empty());
    assert_eq!(summaries[0].transfers.len(), 100);
    let plan = RecoveryPlan::load(&path).unwrap();
    assert_eq!(plan.record().transfers.len(), 120);
    let engine = OperationEngine::default().with_journal_directory(root.join("journals"));
    let retry = finish(
        engine
            .spawn_recovered(plan.claim().unwrap(), options())
            .unwrap(),
    );
    assert_eq!(retry.state, JobState::Done, "{:?}", retry.outcome.errors);
    assert_eq!(fs::read(target.join("file-120")).unwrap(), b"payload 120");
    assert_eq!(
        LocalFs.stat(&last.into(), false).unwrap().identity,
        identity
    );
    assert_eq!(
        read_journal(&path).unwrap().retry,
        retry.outcome.journal_path
    );
    assert_eq!(
        read_journal(retry.outcome.journal_path.as_ref().unwrap())
            .unwrap()
            .transfers
            .len(),
        121
    );
    assert!(
        RecoveryPlan::load(&path).is_err(),
        "an older attempt must not be retried twice"
    );
}

#[test]
fn repeated_restart_retries_preserve_removed_move_roots_and_new_checkpoints() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let target = root.join("target");
    fs::create_dir(&target).unwrap();
    let good = root.join("good");
    let bad = root.join("bad");
    fs::write(&good, b"already moved").unwrap();
    fs::write(&bad, vec![7; 32768]).unwrap();
    let mut broken = FaultFs::new(bad.clone().into(), root.join("trash"));
    broken.failure = Some(Failure::Disconnected);
    broken.cross_device = true;
    let engine =
        OperationEngine::new(Arc::new(broken)).with_journal_directory(root.join("journals"));
    let first = finish(engine.spawn_move(
        vec![good.clone().into(), bad.clone().into()],
        target.clone().into(),
        ScanOptions::default(),
        options(),
    ));
    assert_eq!(first.state, JobState::Failed);
    assert!(!good.exists());
    let plan = RecoveryPlan::load(first.outcome.journal_path.as_ref().unwrap()).unwrap();
    let second = finish(
        engine
            .spawn_recovered(plan.claim().unwrap(), options())
            .unwrap(),
    );
    assert_eq!(second.state, JobState::Failed);
    let second_path = second.outcome.journal_path.unwrap();
    let next = RecoveryPlan::load(&second_path).unwrap();
    assert!(
        next.record()
            .transfers
            .iter()
            .any(|record| record.source == good.clone().into() && record.source_removed)
    );
    let result = finish(
        OperationEngine::default()
            .with_journal_directory(root.join("journals"))
            .spawn_recovered(next.claim().unwrap(), options())
            .unwrap(),
    );
    assert_eq!(result.state, JobState::Done, "{:?}", result.outcome.errors);
    assert!(!bad.exists());
    assert_eq!(fs::read(target.join("good")).unwrap(), b"already moved");
    assert_eq!(fs::read(target.join("bad")).unwrap(), vec![7; 32768]);
}

#[test]
fn reviewed_records_must_be_unchanged_and_exclusively_claimed() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let path = journal(
        root,
        JobKind::Copy,
        vec![root.join("file").into()],
        root.join("target").into(),
        &[],
    );
    let plan = RecoveryPlan::load(&path).unwrap();
    let claim = plan.claim().unwrap();
    assert!(plan.claim().is_err());
    assert!(RecoveryPlan::load(&path).is_err());
    drop(claim);
    journal::mark_reviewed(&path).unwrap();
    assert!(plan.claim().unwrap_err().to_string().contains("changed"));
    assert!(RecoveryPlan::load(&path).unwrap().claim().is_ok());
}

#[test]
fn recovery_does_not_retry_completed_destructive_archive_or_relative_operations() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (kind, sources, destination, state, archive) in [
        (
            JobKind::Copy,
            vec!["/file".into()],
            Some("/target".into()),
            JobState::Done,
            false,
        ),
        (
            JobKind::Trash,
            vec!["/file".into()],
            None,
            JobState::Failed,
            false,
        ),
        (
            JobKind::DeletePermanent,
            vec!["/file".into()],
            None,
            JobState::Cancelled,
            false,
        ),
        (
            JobKind::Copy,
            vec!["/file".into()],
            Some("/target".into()),
            JobState::Failed,
            true,
        ),
        (
            JobKind::Copy,
            vec!["relative".into()],
            Some("/target".into()),
            JobState::Failed,
            false,
        ),
        (
            JobKind::Move,
            vec!["/file".into()],
            Some("relative".into()),
            JobState::Failed,
            false,
        ),
    ] {
        let journal = JobJournal::create(root, kind, sources, destination).unwrap();
        if archive {
            journal
                .intent("update archive", None, &"/target".into())
                .unwrap();
        }
        journal
            .append(&JournalEvent::Finished {
                state,
                errors: vec![],
                completed_items: 0,
            })
            .unwrap();
        let path = journal.path().to_owned();
        assert!(
            RecoveryPlan::load(&path).is_err(),
            "live journal must be excluded"
        );
        drop(journal);
        assert!(
            RecoveryPlan::load(&path).is_err(),
            "ineligible operation was accepted"
        );
    }
}

#[test]
fn truncated_journals_rescan_sources_without_replaying_raw_mutation_intents() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let source = root.join("source");
    let protected = root.join("protected");
    let target = root.join("target");
    fs::write(&source, b"copy me").unwrap();
    fs::write(&protected, b"keep me").unwrap();
    fs::create_dir(&target).unwrap();
    let journal = JobJournal::create(
        &root.join("journals"),
        JobKind::Copy,
        vec![source.clone().into()],
        Some(target.clone().into()),
    )
    .unwrap();
    journal
        .intent(
            "delete permanently",
            Some(&source.into()),
            &protected.clone().into(),
        )
        .unwrap();
    let path = journal.path().to_owned();
    drop(journal);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"event\":\"Trans")
        .unwrap();
    let plan = RecoveryPlan::load(&path).unwrap();
    assert!(plan.record().incomplete_tail);
    let result = finish(
        OperationEngine::default()
            .with_journal_directory(root.join("journals"))
            .spawn_recovered(plan.claim().unwrap(), options())
            .unwrap(),
    );
    assert_eq!(result.state, JobState::Done);
    assert_eq!(fs::read(protected).unwrap(), b"keep me");
    assert_eq!(fs::read(target.join("source")).unwrap(), b"copy me");
    assert_eq!(
        read_journal(&path).unwrap().retry,
        result.outcome.journal_path
    );
}

#[test]
fn journals_keep_multiple_destinations_per_source_with_latest_checkpoint_last() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let source = root.join("source");
    let target = root.join("target");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&target).unwrap();
    let file = source.join("file");
    fs::write(&file, b"shared source").unwrap();
    let result = finish(
        OperationEngine::default()
            .with_journal_directory(root.join("journals"))
            .spawn_copy(
                vec![source.into(), file.clone().into()],
                target.clone().into(),
                ScanOptions::default(),
                options(),
            ),
    );
    assert_eq!(result.state, JobState::Done);
    let record = read_journal(result.outcome.journal_path.as_ref().unwrap()).unwrap();
    let records: Vec<_> = record
        .transfers
        .iter()
        .filter(|record| record.source == file.clone().into())
        .cloned()
        .collect();
    assert_eq!(records.len(), 2, "both outputs must remain recoverable");
    assert_eq!(records[0].destination, target.join("source/file").into());
    assert_eq!(records[1].destination, target.join("file").into());
    let journal = JobJournal::create(
        &root.join("journals"),
        JobKind::Copy,
        vec![file.into()],
        Some(target.into()),
    )
    .unwrap();
    for record in &records {
        journal.transfer(record).unwrap();
    }
    // Re-publishing an earlier destination must replace its checkpoint, not reorder by path.
    journal.transfer(&records[0]).unwrap();
    assert_eq!(
        journal.partial_outcome().transfers,
        vec![records[1].clone(), records[0].clone()]
    );
    let path = journal.path().to_owned();
    drop(journal);
    assert_eq!(
        read_journal(&path).unwrap().transfers,
        vec![records[1].clone(), records[0].clone()]
    );
}

#[test]
fn moved_file_retry_accepts_a_new_inode_only_with_matching_saved_contents() {
    for change in ["same bytes", "corrupt bytes", "legacy record"] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let source = root.join("file");
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(&source, b"original").unwrap();
        let mut cross_device = FaultFs::new(source.clone().into(), root.join("trash"));
        cross_device.cross_device = true;
        let first = finish(OperationEngine::new(Arc::new(cross_device)).spawn_move(
            vec![source.clone().into()],
            target.clone().into(),
            ScanOptions::default(),
            options(),
        ));
        assert_eq!(first.state, JobState::Done);
        let mut records = first.outcome.transfers;
        assert!(records[0].fingerprint.is_some());
        let destination = target.join("file");
        let modified = fs::metadata(&destination).unwrap().modified().unwrap();
        fs::rename(&destination, root.join("old inode")).unwrap();
        fs::write(
            &destination,
            if change == "corrupt bytes" {
                b"changed!"
            } else {
                b"original"
            },
        )
        .unwrap();
        fs::File::open(&destination)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_ne!(
            LocalFs
                .stat(&destination.clone().into(), false)
                .unwrap()
                .identity,
            records[0].metadata.identity
        );
        if change == "legacy record" {
            records[0].fingerprint = None;
        }
        let path = journal(
            root,
            JobKind::Move,
            vec![source.clone().into()],
            target.into(),
            &records,
        );
        let plan = RecoveryPlan::load(&path).unwrap();
        let retry = finish(
            OperationEngine::default()
                .with_journal_directory(root.join("journals"))
                .spawn_recovered(plan.claim().unwrap(), options())
                .unwrap(),
        );
        assert_eq!(
            retry.state,
            if change == "same bytes" {
                JobState::Done
            } else {
                JobState::Failed
            },
            "{change}: {retry:?}"
        );
        assert!(!source.exists());
        assert_eq!(
            fs::read(destination).unwrap(),
            if change == "corrupt bytes" {
                b"changed!"
            } else {
                b"original"
            }
        );
    }
}

#[test]
fn moved_directory_retry_checks_every_saved_child_after_inode_changes() {
    for change in [
        "same tree",
        "extra child",
        "missing child",
        "changed content",
    ] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let source = root.join("folder");
        let target = root.join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(source.join("file"), b"original").unwrap();
        std::os::unix::fs::symlink("file", source.join("link")).unwrap();
        let mut cross_device = FaultFs::new(source.clone().into(), root.join("trash"));
        cross_device.cross_device = true;
        let first = finish(OperationEngine::new(Arc::new(cross_device)).spawn_move(
            vec![source.clone().into()],
            target.clone().into(),
            ScanOptions::default(),
            options(),
        ));
        assert_eq!(first.state, JobState::Done);
        let destination = target.join("folder");
        fs::rename(&destination, root.join("old tree")).unwrap();
        fs::create_dir(&destination).unwrap();
        // Reuse the actual leaf objects to preserve symlink timestamps; the parent
        // changes identity. Contents still need proof before accepting the new tree.
        fs::rename(root.join("old tree/file"), destination.join("file")).unwrap();
        fs::rename(root.join("old tree/link"), destination.join("link")).unwrap();
        match change {
            "extra child" => fs::write(destination.join("extra"), b"new").unwrap(),
            "missing child" => fs::remove_file(destination.join("file")).unwrap(),
            "changed content" => {
                let path = destination.join("file");
                let modified = fs::metadata(&path).unwrap().modified().unwrap();
                fs::write(&path, b"changed!").unwrap();
                fs::File::open(&path)
                    .unwrap()
                    .set_modified(modified)
                    .unwrap();
            }
            _ => {}
        }
        let path = journal(
            root,
            JobKind::Move,
            vec![source.clone().into()],
            target.into(),
            &first.outcome.transfers,
        );
        let plan = RecoveryPlan::load(&path).unwrap();
        let retry = finish(
            OperationEngine::default()
                .with_journal_directory(root.join("journals"))
                .spawn_recovered(plan.claim().unwrap(), options())
                .unwrap(),
        );
        assert_eq!(
            retry.state,
            if change == "same tree" {
                JobState::Done
            } else {
                JobState::Failed
            },
            "{change}: {retry:?}"
        );
        assert!(!source.exists());
        assert!(destination.join("link").is_symlink());
    }
}
