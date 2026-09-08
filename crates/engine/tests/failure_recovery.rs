use dualpane_core::VPath;
use dualpane_engine::*;
use std::{
    fs,
    sync::{Arc, atomic::Ordering},
};

#[path = "../../vfs/tests/support/fault_fs.rs"]
mod fault_fs;
use fault_fs::{Failure, FaultFs};

fn options() -> TransferOptions {
    TransferOptions {
        conflict_policy: ConflictPolicy::Overwrite,
        verify: true,
        durable: true,
        parallel: false,
    }
}

#[test]
fn readable_nonseekable_sources_copy_and_verify_successfully() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("source.txt");
    let destination = fixture.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let content = vec![b'N'; 128 * 1024];
    fs::write(&source, &content).unwrap();
    let mut vfs = FaultFs::new(source.clone().into(), fixture.path().join("trash"));
    vfs.nonseekable = true;
    let engine =
        OperationEngine::new(Arc::new(vfs)).with_journal_directory(fixture.path().join("journals"));
    let summary = engine
        .spawn_copy(
            vec![source.into()],
            destination.clone().into(),
            ScanOptions::default(),
            options(),
        )
        .join();
    assert_eq!(summary.state, JobState::Done, "{summary:?}");
    assert_eq!(fs::read(destination.join("source.txt")).unwrap(), content);
}

#[test]
fn failed_copy_and_cross_device_move_keep_sources_and_original_destinations() {
    for moving in [false, true] {
        for failure in [
            Failure::DiskFull,
            Failure::Disconnected,
            Failure::Flush,
            Failure::Sync,
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let source = fixture.path().join("source.txt");
            let destination = fixture.path().join("destination");
            fs::create_dir(&destination).unwrap();
            fs::write(&source, vec![b'N'; 2 * 1024 * 1024]).unwrap();
            fs::write(destination.join("source.txt"), b"original").unwrap();
            let mut vfs = FaultFs::new(VPath::from(source.clone()), fixture.path().join("trash"));
            vfs.failure = Some(failure);
            vfs.cross_device = moving;
            let triggered = Arc::clone(&vfs.triggered);
            let engine = OperationEngine::new(Arc::new(vfs))
                .with_journal_directory(fixture.path().join("journals"));
            let handle = if moving {
                engine.spawn_move(
                    vec![source.clone().into()],
                    destination.clone().into(),
                    ScanOptions::default(),
                    options(),
                )
            } else {
                engine.spawn_copy(
                    vec![source.clone().into()],
                    destination.clone().into(),
                    ScanOptions::default(),
                    options(),
                )
            };
            let summary = handle.join();
            assert!(
                triggered.load(Ordering::SeqCst),
                "fault was not exercised: {failure:?}"
            );
            assert_eq!(summary.state, JobState::Failed, "{failure:?}");
            assert_eq!(fs::read(&source).unwrap(), vec![b'N'; 2 * 1024 * 1024]);
            assert_eq!(
                fs::read(destination.join("source.txt")).unwrap(),
                b"original"
            );
            assert_eq!(
                fs::read_dir(&destination).unwrap().count(),
                1,
                "partial copies must be cleaned up"
            );
            let (records, errors) = journal::scan_journals(&fixture.path().join("journals"));
            assert!(errors.is_empty());
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].state, Some(JobState::Failed));
        }
    }
}

#[test]
fn cancellation_before_publication_keeps_the_original() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("source.txt");
    let dest = fixture.path().join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&source, vec![b'N'; 2 * 1024 * 1024]).unwrap();
    fs::write(dest.join("source.txt"), b"original").unwrap();
    let control = JobControl::new();
    let mut vfs = FaultFs::new(source.clone().into(), fixture.path().join("trash"));
    vfs.failure = Some(Failure::Cancel);
    vfs.cancel = control.cancel_token();
    let plan = scan_sources(
        &vfs,
        &[source.clone().into()],
        ScanOptions::default(),
        &control,
        |_| {},
    )
    .unwrap();
    let result = copy_plan(
        &vfs,
        &plan,
        &dest.clone().into(),
        options(),
        &control,
        |_| {},
        |_| {},
        |_| panic!("unexpected conflict"),
    );
    assert!(result.is_err());
    assert!(vfs.triggered.load(Ordering::SeqCst));
    assert_eq!(fs::read(dest.join("source.txt")).unwrap(), b"original");
    assert!(source.exists());
}

#[test]
fn interrupted_process_leaves_a_readable_recovery_record_and_can_be_retried() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("source.txt");
    let dest = fixture.path().join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&source, vec![b'N'; 2 * 1024 * 1024]).unwrap();
    fs::write(dest.join("source.txt"), b"original").unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_fixture_child", "--nocapture"])
        .env("COMMANDER_CRASH_FIXTURE", fixture.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(77));
    assert_eq!(fs::read(dest.join("source.txt")).unwrap(), b"original");
    assert!(source.exists());
    let (records, errors) = journal::scan_journals(&fixture.path().join("journals"));
    assert!(errors.is_empty());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, None);
    assert!(!records[0].pending.is_empty());
    let engine = OperationEngine::default().with_journal_directory(fixture.path().join("journals"));
    let summary = engine
        .spawn_copy(
            vec![source.clone().into()],
            dest.clone().into(),
            ScanOptions::default(),
            options(),
        )
        .join();
    assert_eq!(summary.state, JobState::Done);
    assert_eq!(
        fs::read(dest.join("source.txt")).unwrap(),
        fs::read(source).unwrap()
    );
    assert_eq!(summary.outcome.backups.len(), 1);
    assert_eq!(
        fs::read(summary.outcome.backups[0].backup.as_path()).unwrap(),
        b"original"
    );
}

#[test]
fn crash_fixture_child() {
    let Some(root) = std::env::var_os("COMMANDER_CRASH_FIXTURE").map(std::path::PathBuf::from)
    else {
        return;
    };
    let mut vfs = FaultFs::new(root.join("source.txt").into(), root.join("trash"));
    vfs.failure = Some(Failure::Crash);
    let engine = OperationEngine::new(Arc::new(vfs)).with_journal_directory(root.join("journals"));
    let _ = engine
        .spawn_copy(
            vec![root.join("source.txt").into()],
            root.join("dest").into(),
            ScanOptions::default(),
            options(),
        )
        .join();
    panic!("crash injection was not reached");
}
