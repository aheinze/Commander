use super::*;
use dualpane_vfs::LocalFs;
use std::{
    fs,
    sync::{Arc, atomic::Ordering},
};

#[allow(dead_code)]
#[path = "../../../vfs/tests/support/fault_fs.rs"]
mod fault_fs;
use fault_fs::{Failure, FaultFs};

fn folders() -> (tempfile::TempDir, VPath, VPath) {
    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("left");
    let right = fixture.path().join("right");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    (fixture, left.into(), right.into())
}
fn write(root: &VPath, name: &str, content: impl AsRef<[u8]>) {
    let path = root.as_path().join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
fn plan(left: &VPath, right: &VPath, mirror: bool, verified: bool) -> SyncPlan {
    plan_sync(
        left.clone(),
        right.clone(),
        compare_with_contents(&LocalFs, left, right, verified, &CancelToken::new()).unwrap(),
        SyncDirection::LeftToRight,
        mirror,
        verified,
    )
    .unwrap()
}
fn same_time(left: &VPath, right: &VPath) {
    let stamp = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for path in [left, right] {
        fs::File::options()
            .write(true)
            .open(path.as_path())
            .unwrap()
            .set_modified(stamp)
            .unwrap();
    }
}

#[test]
fn content_comparison_detects_equal_metadata_with_different_bytes() {
    let (_fixture, left, right) = folders();
    write(&left, "same.txt", "AAAA");
    write(&right, "same.txt", "BBBB");
    same_time(
        &left.join_name("same.txt".as_ref()),
        &right.join_name("same.txt".as_ref()),
    );
    assert_eq!(
        compare_directories(&LocalFs, &left, &right, &CancelToken::new()).unwrap()[0].status,
        CompareStatus::Same
    );
    let checked =
        compare_with_contents(&LocalFs, &left, &right, true, &CancelToken::new()).unwrap();
    assert_eq!(checked[0].status, CompareStatus::Different);
    assert_ne!(checked[0].left_digest, checked[0].right_digest);
    write(&right, "same.txt", "AAAA");
    assert_eq!(
        compare_with_contents(&LocalFs, &left, &right, true, &CancelToken::new()).unwrap()[0]
            .status,
        CompareStatus::Same
    );
}

#[test]
fn directories_compare_by_children_and_symlinks_by_targets() {
    let (_fixture, left, right) = folders();
    write(&left, "folder/a", "same");
    write(&right, "folder/a", "same");
    fs::File::open(left.as_path().join("folder"))
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH)
        .unwrap();
    std::os::unix::fs::symlink("aaa", left.as_path().join("link")).unwrap();
    std::os::unix::fs::symlink("bbb", right.as_path().join("link")).unwrap();
    let entries =
        compare_with_contents(&LocalFs, &left, &right, true, &CancelToken::new()).unwrap();
    assert_eq!(entries[0].status, CompareStatus::Same);
    assert_eq!(entries[1].status, CompareStatus::Same);
    assert_eq!(entries[2].status, CompareStatus::Different);
}

#[test]
fn reviewed_sync_copies_replaces_and_trashes_only_destination_extras_in_both_directions() {
    for reverse in [false, true] {
        let (fixture, left, right) = folders();
        let (source, destination) = if reverse {
            (&right, &left)
        } else {
            (&left, &right)
        };
        write(source, "replace.txt", "new content");
        write(destination, "replace.txt", "original");
        write(source, "new/deep/file.txt", "new nested");
        write(destination, "extra/deep/old.txt", "keep in trash");
        fs::create_dir(source.as_path().join("empty")).unwrap();
        std::os::unix::fs::symlink("replace.txt", source.as_path().join("link")).unwrap();
        let mut plan = plan(&left, &right, true, true);
        if reverse {
            plan = plan_sync(
                left.clone(),
                right.clone(),
                plan.entries,
                SyncDirection::RightToLeft,
                true,
                true,
            )
            .unwrap();
        }
        assert!(!plan.blocked());
        assert_eq!(
            plan.actions
                .iter()
                .filter(|action| action.kind == SyncActionKind::Trash)
                .count(),
            1
        );
        let vfs = Arc::new(FaultFs::new(
            source.join_name("replace.txt".as_ref()),
            fixture.path().join("trash"),
        ));
        let engine =
            OperationEngine::new(vfs.clone()).with_journal_directory(fixture.path().join("jobs"));
        let count = execute_sync_plan(vfs.as_ref(), &engine, &plan, &CancelToken::new(), |_, _| {})
            .unwrap();
        assert_eq!(count, plan.actions.len());
        assert_eq!(
            fs::read(destination.as_path().join("replace.txt")).unwrap(),
            b"new content"
        );
        assert_eq!(
            fs::read(destination.as_path().join("new/deep/file.txt")).unwrap(),
            b"new nested"
        );
        assert_eq!(
            fs::read_link(destination.as_path().join("link")).unwrap(),
            Path::new("replace.txt")
        );
        assert!(destination.as_path().join("empty").is_dir());
        assert!(!destination.as_path().join("extra").exists());
        assert!(
            fixture
                .path()
                .join("trash/files/extra/deep/old.txt")
                .exists()
        );
        assert!(
            compare_with_contents(&LocalFs, &left, &right, true, &CancelToken::new())
                .unwrap()
                .iter()
                .all(|entry| entry.status == CompareStatus::Same)
        );
        let (records, errors) =
            dualpane_engine::journal::scan_journals(&fixture.path().join("jobs"));
        assert!(errors.is_empty());
        assert!(
            records
                .iter()
                .flat_map(|record| &record.backups)
                .any(|backup| fs::read(backup.backup.as_path()).unwrap() == b"original")
        );
    }
}

#[test]
fn stale_comparisons_do_not_apply_any_changes() {
    for side in [true, false] {
        let (_fixture, left, right) = folders();
        write(&left, "file", "new");
        write(&right, "file", "old");
        let plan = plan(&left, &right, true, false);
        write(
            if side { &left } else { &right },
            "added-after-review",
            "unreviewed",
        );
        let error = execute_sync_plan(
            &LocalFs,
            &OperationEngine::default(),
            &plan,
            &CancelToken::new(),
            |_, _| {},
        )
        .unwrap_err();
        assert!(error.contains("no changes were applied"));
        assert_eq!(fs::read(right.as_path().join("file")).unwrap(), b"old");
    }
}

#[test]
fn verified_sync_rechecks_same_metadata_edits_immediately_before_replacement() {
    for mutate_source in [true, false] {
        let (_fixture, left, right) = folders();
        write(&left, "file", "AAAA");
        write(&right, "file", "BBBB");
        let source = left.join_name("file".as_ref());
        let target = right.join_name("file".as_ref());
        same_time(&source, &target);
        let plan = plan(&left, &right, false, true);
        let error = execute_sync_plan(
            &LocalFs,
            &OperationEngine::default(),
            &plan,
            &CancelToken::new(),
            |_, message| {
                if message.starts_with("Replace:") {
                    write(if mutate_source { &left } else { &right }, "file", "XXXX");
                    same_time(&source, &target);
                }
            },
        )
        .unwrap_err();
        assert!(error.contains("since review"));
        assert_eq!(
            fs::read(target.as_path()).unwrap(),
            if mutate_source { b"BBBB" } else { b"XXXX" }
        );
    }
}

#[test]
fn mirror_rechecks_nested_children_before_moving_a_folder_to_trash() {
    let (fixture, left, right) = folders();
    write(&right, "extra/nested/file", "old");
    let plan = plan(&left, &right, true, false);
    let vfs = Arc::new(FaultFs::new(left.clone(), fixture.path().join("trash")));
    let engine = OperationEngine::new(vfs.clone());
    let error = execute_sync_plan(
        vfs.as_ref(),
        &engine,
        &plan,
        &CancelToken::new(),
        |_, message| {
            if message.starts_with("Move to Trash:") {
                write(&right, "extra/nested/file", "changed after preflight");
            }
        },
    )
    .unwrap_err();
    assert!(error.contains("since review"));
    assert!(right.as_path().join("extra/nested/file").exists());
    assert!(!fixture.path().join("trash/files/extra").exists());
}

#[test]
fn failed_sync_replacements_preserve_originals_and_do_not_start_mirror_cleanup() {
    for failure in [
        Failure::DiskFull,
        Failure::Disconnected,
        Failure::Flush,
        Failure::Sync,
    ] {
        let (fixture, left, right) = folders();
        write(&left, "file", vec![b'N'; 2 * 1024 * 1024]);
        write(&right, "file", "original");
        write(&right, "extra", "must remain");
        let plan = plan(&left, &right, true, false);
        let mut vfs = FaultFs::new(
            left.join_name("file".as_ref()),
            fixture.path().join("trash"),
        );
        vfs.failure = Some(failure);
        let vfs = Arc::new(vfs);
        let engine =
            OperationEngine::new(vfs.clone()).with_journal_directory(fixture.path().join("jobs"));
        let error = execute_sync_plan(vfs.as_ref(), &engine, &plan, &CancelToken::new(), |_, _| {})
            .unwrap_err();
        assert!(vfs.triggered.load(Ordering::SeqCst));
        assert!(error.contains("Stopped after 0"));
        assert_eq!(fs::read(right.as_path().join("file")).unwrap(), b"original");
        assert!(right.as_path().join("extra").exists());
        assert_eq!(fs::read_dir(right.as_path()).unwrap().count(), 2);
    }
}

#[test]
fn cancelled_invalid_overlapping_and_type_conflict_plans_do_not_mutate() {
    let (_fixture, left, right) = folders();
    write(&left, "file/child", "source");
    write(&right, "file", "original");
    let plan = plan(&left, &right, true, false);
    assert!(plan.blocked());
    assert!(
        execute_sync_plan(
            &LocalFs,
            &OperationEngine::default(),
            &plan,
            &CancelToken::new(),
            |_, _| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(right.as_path().join("file")).unwrap(), b"original");
    for destination in [left.clone(), left.join_name("file".as_ref())] {
        let mut overlapping = plan.clone();
        overlapping.actions.clear();
        overlapping.right = destination;
        assert!(
            execute_sync_plan(
                &LocalFs,
                &OperationEngine::default(),
                &overlapping,
                &CancelToken::new(),
                |_, _| {}
            )
            .unwrap_err()
            .contains("separate folders")
        );
    }
    let mut invalid = plan.clone();
    invalid.actions[0].relative_path = "../outside".into();
    assert!(
        execute_sync_plan(
            &LocalFs,
            &OperationEngine::default(),
            &invalid,
            &CancelToken::new(),
            |_, _| {}
        )
        .unwrap_err()
        .contains("invalid relative path")
    );
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(
        execute_sync_plan(
            &LocalFs,
            &OperationEngine::default(),
            &plan,
            &cancel,
            |_, _| {}
        )
        .unwrap_err()
        .contains("cancelled")
    );
}

#[test]
fn redirected_destination_parents_cannot_receive_unreviewed_writes() {
    let (fixture, left, right) = folders();
    write(&left, "folder/new-file", "source");
    fs::create_dir(right.as_path().join("folder")).unwrap();
    let outside = fixture.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let plan = plan(&left, &right, false, false);
    let error = execute_sync_plan(
        &LocalFs,
        &OperationEngine::default(),
        &plan,
        &CancelToken::new(),
        |_, message| {
            if message.starts_with("Copy new:") {
                fs::remove_dir(right.as_path().join("folder")).unwrap();
                std::os::unix::fs::symlink(&outside, right.as_path().join("folder")).unwrap();
            }
        },
    )
    .unwrap_err();
    assert!(error.contains("no longer a directory"));
    assert!(!outside.join("new-file").exists());
}

#[test]
fn mirror_cannot_trash_a_folder_containing_excluded_recovery_backups() {
    let (fixture, left, right) = folders();
    let backup = "extra/.commander-undo-123abc-1";
    write(&right, backup, "recoverable original");
    let plan = plan(&left, &right, true, true);
    assert_eq!(plan.actions.len(), 1);
    let vfs = Arc::new(FaultFs::new(left.clone(), fixture.path().join("trash")));
    let engine =
        OperationEngine::new(vfs.clone()).with_journal_directory(fixture.path().join("journals"));
    let error = execute_sync_plan(vfs.as_ref(), &engine, &plan, &CancelToken::new(), |_, _| {})
        .unwrap_err();
    assert!(error.contains("contains recovery files"));
    assert_eq!(
        fs::read(right.as_path().join(backup)).unwrap(),
        b"recoverable original"
    );
}
