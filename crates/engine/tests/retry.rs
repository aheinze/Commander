use dualpane_core::{EntryKind, VPath};
use dualpane_engine::*;
use dualpane_vfs::{LocalFs, Vfs};
use std::{fs, sync::Arc, time::Duration};
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
fn finish_without_conflicts(job: JobHandle) -> JobSummary {
    loop {
        match job
            .events()
            .recv_timeout(Duration::from_secs(10))
            .expect("job stalled")
        {
            JobEvent::Conflict { conflict, .. } => {
                job.cancel();
                panic!("completed file asked for overwrite: {conflict:?}");
            }
            JobEvent::Finished { .. } => break,
            _ => {}
        }
    }
    job.join()
}

#[test]
fn retry_copy_reuses_verified_nested_files_and_completes_failed_siblings() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("source");
    let target = fixture.path().join("target");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(source.join("nested/good"), b"already copied").unwrap();
    let bad = source.join("nested/bad");
    fs::write(&bad, vec![7; 32 * 1024]).unwrap();
    let mut broken = FaultFs::new(bad.clone().into(), fixture.path().join("trash"));
    broken.failure = Some(Failure::Disconnected);
    let engine = OperationEngine::new(Arc::new(broken))
        .with_journal_directory(fixture.path().join("journals"));
    let first = finish_without_conflicts(engine.spawn_copy(
        vec![source.clone().into()],
        target.clone().into(),
        ScanOptions::default(),
        options(),
    ));
    assert_eq!(first.state, JobState::Failed);
    let committed = target.join("source/nested/good");
    let before = LocalFs.stat(&committed.clone().into(), false).unwrap();
    let retry = finish_without_conflicts(OperationEngine::default().spawn_copy_retry(
        vec![source.into()],
        target.into(),
        ScanOptions::default(),
        options(),
        first.outcome.transfers,
    ));
    assert_eq!(retry.state, JobState::Done, "{:?}", retry.outcome.errors);
    assert_eq!(
        LocalFs
            .stat(&committed.clone().into(), false)
            .unwrap()
            .identity,
        before.identity
    );
    assert_eq!(
        fs::read(committed.parent().unwrap().join("bad")).unwrap(),
        vec![7; 32 * 1024]
    );
}

#[test]
fn retry_never_skips_changed_content_even_with_the_same_size_and_timestamp() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("file");
    let target = fixture.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(&source, b"original").unwrap();
    let engine = OperationEngine::default();
    let first = finish_without_conflicts(engine.spawn_copy(
        vec![source.clone().into()],
        target.clone().into(),
        ScanOptions::default(),
        options(),
    ));
    let copied = target.join("file");
    let modified = fs::metadata(&copied).unwrap().modified().unwrap();
    fs::write(&copied, b"changed!").unwrap();
    fs::File::open(&copied)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let job = engine.spawn_copy_retry(
        vec![source.into()],
        target.into(),
        ScanOptions::default(),
        options(),
        first.outcome.transfers,
    );
    loop {
        match job.events().recv_timeout(Duration::from_secs(10)).unwrap() {
            JobEvent::Conflict { conflict_id, .. } => {
                job.resolve_conflict(
                    conflict_id,
                    ConflictDecision {
                        action: ConflictAction::Skip,
                        apply_to_all: false,
                    },
                )
                .unwrap();
                break;
            }
            JobEvent::Finished { .. } => panic!("changed destination was silently reused"),
            _ => {}
        }
    }
    assert_eq!(job.join().state, JobState::Done);
    assert_eq!(fs::read(copied).unwrap(), b"changed!");
}

#[test]
fn retry_preserves_keep_both_destinations_and_missing_completed_move_roots() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("file");
    let target = fixture.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(&source, b"source").unwrap();
    fs::write(target.join("file"), b"preexisting").unwrap();
    let engine = OperationEngine::default();
    let first = engine
        .spawn_copy(
            vec![source.clone().into()],
            target.clone().into(),
            ScanOptions::default(),
            TransferOptions {
                conflict_policy: ConflictPolicy::Rename,
                ..options()
            },
        )
        .join();
    let destination = first.outcome.transfers[0].destination.clone();
    assert_ne!(destination, target.join("file").into());
    let retried = finish_without_conflicts(engine.spawn_move_retry(
        vec![source.clone().into()],
        target.clone().into(),
        ScanOptions::default(),
        options(),
        first.outcome.transfers,
    ));
    assert_eq!(
        retried.state,
        JobState::Done,
        "{:?}",
        retried.outcome.errors
    );
    assert!(!source.exists());
    assert_eq!(fs::read(destination.as_path()).unwrap(), b"source");
    assert_eq!(fs::read(target.join("file")).unwrap(), b"preexisting");
    let second = fixture.path().join("second");
    fs::write(&second, b"remaining").unwrap();
    let retried = finish_without_conflicts(engine.spawn_move_retry(
        vec![source.into(), second.clone().into()],
        target.clone().into(),
        ScanOptions::default(),
        options(),
        retried.outcome.transfers,
    ));
    assert_eq!(
        retried.state,
        JobState::Done,
        "{:?}",
        retried.outcome.errors
    );
    assert!(!second.exists());
    assert_eq!(fs::read(target.join("second")).unwrap(), b"remaining");
}

#[test]
fn retry_does_not_reuse_a_symlink_with_a_different_target() {
    let fixture = tempfile::tempdir().unwrap();
    let source: VPath = fixture.path().join("link").into();
    let target: VPath = fixture.path().join("target").into();
    fs::create_dir(target.as_path()).unwrap();
    LocalFs.create_symlink(&"/old".into(), &source).unwrap();
    let engine = OperationEngine::default();
    let first = finish_without_conflicts(engine.spawn_copy(
        vec![source.clone()],
        target.clone(),
        ScanOptions::default(),
        options(),
    ));
    LocalFs.remove(&source, EntryKind::Symlink).unwrap();
    LocalFs.create_symlink(&"/new".into(), &source).unwrap();
    let job = engine.spawn_copy_retry(
        vec![source],
        target.clone(),
        ScanOptions::default(),
        TransferOptions {
            conflict_policy: ConflictPolicy::Overwrite,
            ..options()
        },
        first.outcome.transfers,
    );
    assert_eq!(job.join().state, JobState::Done);
    assert_eq!(
        fs::read_link(target.as_path().join("link")).unwrap(),
        std::path::PathBuf::from("/new")
    );
}
