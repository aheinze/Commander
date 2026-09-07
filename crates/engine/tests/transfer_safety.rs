use dualpane_core::VPath;
use dualpane_engine::*;
use dualpane_vfs::{LocalFs, Vfs};
use std::fs;
use tempfile::tempdir;

fn vp(path: impl AsRef<std::path::Path>) -> VPath {
    VPath::from(path.as_ref())
}
fn opts(policy: ConflictPolicy) -> TransferOptions {
    TransferOptions {
        conflict_policy: policy,
        parallel: false,
        ..TransferOptions::default()
    }
}
fn reject_ask(_: &Conflict) -> Result<ConflictDecision, Cancelled> {
    panic!("unexpected prompt")
}

#[test]
fn moving_to_same_directory_must_preserve_source_on_skip() {
    let f = tempdir().unwrap();
    let source = f.path().join("important.txt");
    fs::write(&source, b"irreplaceable").unwrap();
    let outcome = move_sources(
        &LocalFs,
        &[vp(&source)],
        &vp(f.path()),
        ScanOptions::default(),
        opts(ConflictPolicy::Skip),
        &JobControl::new(),
        |_| {},
        |_| {},
        reject_ask,
    )
    .unwrap();
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert!(
        source.exists(),
        "Move+Skip deleted the original file in the same directory"
    );
}

#[test]
fn skipped_move_must_preserve_source_when_metadata_matches() {
    let f = tempdir().unwrap();
    let destination = f.path().join("dest");
    fs::create_dir(&destination).unwrap();
    let source = f.path().join("important.txt");
    let target = destination.join("important.txt");
    fs::write(&source, b"AAAA").unwrap();
    fs::write(&target, b"BBBB").unwrap();
    let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(timestamp)
        .unwrap();
    fs::File::options()
        .write(true)
        .open(&target)
        .unwrap()
        .set_modified(timestamp)
        .unwrap();
    let outcome = move_sources(
        &LocalFs,
        &[vp(&source)],
        &vp(&destination),
        ScanOptions::default(),
        opts(ConflictPolicy::Skip),
        &JobControl::new(),
        |_| {},
        |_| {},
        reject_ask,
    )
    .unwrap();
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert!(
        source.exists(),
        "Skipped move deleted AAAA while the destination still contains BBBB"
    );
}

#[test]
fn keep_both_apply_all_must_create_distinct_outputs() {
    let f = tempdir().unwrap();
    let dest = f.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let a = f.path().join("a.txt");
    let b = f.path().join("b.txt");
    fs::write(&a, b"new A").unwrap();
    fs::write(&b, b"new B").unwrap();
    fs::write(dest.join("a.txt"), b"old A").unwrap();
    fs::write(dest.join("b.txt"), b"old B").unwrap();
    let control = JobControl::new();
    let plan = scan_sources(
        &LocalFs,
        &[vp(&a), vp(&b)],
        ScanOptions::default(),
        &control,
        |_| {},
    )
    .unwrap();
    let mut prompts = 0;
    let outcome = copy_plan(
        &LocalFs,
        &plan,
        &vp(&dest),
        opts(ConflictPolicy::Ask),
        &control,
        |_| {},
        |_| {},
        |conflict| {
            prompts += 1;
            Ok(ConflictDecision {
                action: ConflictAction::Rename(unique_renamed_path(&conflict.destination, |p| {
                    p.as_path().exists()
                })),
                apply_to_all: true,
            })
        },
    )
    .unwrap();
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(prompts, 1);
    assert_eq!(
        fs::read(dest.join("a (2).txt")).unwrap(),
        b"new A",
        "second file overwrote first Keep Both output"
    );
    assert_eq!(fs::read(dest.join("b (2).txt")).unwrap(), b"new B");
}

#[test]
fn replacing_file_with_directory_must_copy_children() {
    let f = tempdir().unwrap();
    let source = f.path().join("folder");
    let dest = f.path().join("dest");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::write(source.join("child.txt"), b"child").unwrap();
    fs::write(dest.join("folder"), b"old file").unwrap();
    let control = JobControl::new();
    let plan = scan_sources(
        &LocalFs,
        &[vp(&source)],
        ScanOptions::default(),
        &control,
        |_| {},
    )
    .unwrap();
    let outcome = copy_plan(
        &LocalFs,
        &plan,
        &vp(&dest),
        opts(ConflictPolicy::Ask),
        &control,
        |_| {},
        |_| {},
        |_| {
            Ok(ConflictDecision {
                action: ConflictAction::Overwrite,
                apply_to_all: false,
            })
        },
    )
    .unwrap();
    assert!(
        dest.join("folder/child.txt").exists(),
        "confirmed folder replacement left an empty folder; errors={:?}",
        outcome.errors
    );
}

#[test]
fn move_keep_both_removes_only_the_source_and_uses_the_renamed_target() {
    let f = tempdir().unwrap();
    let dest = f.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let source = f.path().join("file.txt");
    fs::write(&source, b"new payload").unwrap();
    fs::write(dest.join("file.txt"), b"old").unwrap();
    let result = move_sources(
        &LocalFs,
        &[vp(&source)],
        &vp(&dest),
        ScanOptions::default(),
        opts(ConflictPolicy::Rename),
        &JobControl::new(),
        |_| {},
        |_| {},
        reject_ask,
    )
    .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(!source.exists());
    assert_eq!(fs::read(dest.join("file.txt")).unwrap(), b"old");
    assert_eq!(fs::read(dest.join("file (2).txt")).unwrap(), b"new payload");
    assert!(result.transfers.iter().all(|record| record.source_removed));
}

#[test]
fn move_directory_merge_retains_skipped_children() {
    let f = tempdir().unwrap();
    let source = f.path().join("folder");
    let dest = f.path().join("dest");
    fs::create_dir(&source).unwrap();
    fs::create_dir_all(dest.join("folder")).unwrap();
    fs::write(source.join("skip"), b"source").unwrap();
    fs::write(source.join("move"), b"moved").unwrap();
    fs::write(dest.join("folder/skip"), b"existing").unwrap();
    let result = move_sources(
        &LocalFs,
        &[vp(&source)],
        &vp(&dest),
        ScanOptions::default(),
        opts(ConflictPolicy::Skip),
        &JobControl::new(),
        |_| {},
        |_| {},
        reject_ask,
    )
    .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(fs::read(source.join("skip")).unwrap(), b"source");
    assert!(!source.join("move").exists());
    assert_eq!(fs::read(dest.join("folder/move")).unwrap(), b"moved");
}

#[test]
fn directory_conflicts_skip_or_keep_both_the_entire_subtree() {
    for policy in [ConflictPolicy::Skip, ConflictPolicy::Rename] {
        let f = tempdir().unwrap();
        let source = f.path().join("folder");
        let dest = f.path().join("dest");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(source.join("nested/child"), b"child").unwrap();
        fs::write(dest.join("folder"), b"existing file").unwrap();
        let control = JobControl::new();
        let plan = scan_sources(
            &LocalFs,
            &[vp(&source)],
            ScanOptions::default(),
            &control,
            |_| {},
        )
        .unwrap();
        let result = copy_plan(
            &LocalFs,
            &plan,
            &vp(&dest),
            opts(policy),
            &control,
            |_| {},
            |_| {},
            reject_ask,
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(fs::read(dest.join("folder")).unwrap(), b"existing file");
        if policy == ConflictPolicy::Rename {
            assert_eq!(
                fs::read(dest.join("folder (2)/nested/child")).unwrap(),
                b"child"
            );
        } else {
            assert!(result.transfers.is_empty());
        }
    }
}

#[test]
fn conditional_apply_all_is_evaluated_for_each_conflict() {
    let f = tempdir().unwrap();
    let dest = f.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let a = f.path().join("a");
    let b = f.path().join("b");
    fs::write(&a, b"new A").unwrap();
    fs::write(&b, b"old B").unwrap();
    fs::write(dest.join("a"), b"old A").unwrap();
    fs::write(dest.join("b"), b"new B").unwrap();
    for (path, seconds) in [(&a, 2), (&b, 1), (&dest.join("a"), 1), (&dest.join("b"), 2)] {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
            .unwrap();
    }
    let control = JobControl::new();
    let plan = scan_sources(
        &LocalFs,
        &[vp(&a), vp(&b)],
        ScanOptions::default(),
        &control,
        |_| {},
    )
    .unwrap();
    let mut prompts = 0;
    let result = copy_plan(
        &LocalFs,
        &plan,
        &vp(&dest),
        opts(ConflictPolicy::Ask),
        &control,
        |_| {},
        |_| {},
        |_| {
            prompts += 1;
            Ok(ConflictDecision {
                action: ConflictAction::OverwriteIfNewer,
                apply_to_all: true,
            })
        },
    )
    .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(prompts, 1);
    assert_eq!(fs::read(dest.join("a")).unwrap(), b"new A");
    assert_eq!(fs::read(dest.join("b")).unwrap(), b"new B");
}

#[test]
fn keep_both_refuses_a_destination_created_while_the_dialog_is_open() {
    let f = tempdir().unwrap();
    let dest = f.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let source = f.path().join("file");
    fs::write(&source, b"source").unwrap();
    fs::write(dest.join("file"), b"existing").unwrap();
    let alternate = dest.join("file (2)");
    let control = JobControl::new();
    let plan = scan_sources(
        &LocalFs,
        &[vp(&source)],
        ScanOptions::default(),
        &control,
        |_| {},
    )
    .unwrap();
    let result = copy_plan(
        &LocalFs,
        &plan,
        &vp(&dest),
        opts(ConflictPolicy::Ask),
        &control,
        |_| {},
        |_| {},
        |_| {
            fs::write(&alternate, b"concurrent file").unwrap();
            Ok(ConflictDecision {
                action: ConflictAction::Rename(vp(&alternate)),
                apply_to_all: false,
            })
        },
    )
    .unwrap();
    assert!(!result.errors.is_empty());
    assert_eq!(fs::read(&alternate).unwrap(), b"concurrent file");
    assert_eq!(fs::read(&source).unwrap(), b"source");
}

#[test]
fn copying_or_moving_into_a_descendant_is_rejected() {
    let f = tempdir().unwrap();
    let source = f.path().join("folder");
    let nested = source.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(source.join("keep"), b"keep").unwrap();
    let control = JobControl::new();
    let plan = scan_sources(
        &LocalFs,
        &[vp(&source)],
        ScanOptions::default(),
        &control,
        |_| {},
    )
    .unwrap();
    let copied = copy_plan(
        &LocalFs,
        &plan,
        &vp(&nested),
        opts(ConflictPolicy::Overwrite),
        &control,
        |_| {},
        |_| {},
        reject_ask,
    )
    .unwrap();
    let moved = move_sources(
        &LocalFs,
        &[vp(&source)],
        &vp(&nested),
        ScanOptions::default(),
        opts(ConflictPolicy::Overwrite),
        &control,
        |_| {},
        |_| {},
        reject_ask,
    )
    .unwrap();
    assert!(!copied.errors.is_empty());
    assert!(!moved.errors.is_empty());
    assert!(!nested.join("folder").exists());
    assert_eq!(fs::read(source.join("keep")).unwrap(), b"keep");
}

#[test]
fn verification_rejects_equal_metadata_with_different_bytes() {
    let f = tempdir().unwrap();
    let a = f.path().join("a");
    let b = f.path().join("b");
    fs::write(&a, b"AAAA").unwrap();
    fs::write(&b, b"BBBB").unwrap();
    for path in [&a, &b] {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH)
            .unwrap();
    }
    assert!(verify_copy(&LocalFs, &vp(&a), &vp(&b)).is_err());
}

#[test]
fn failed_jobs_are_reported_as_failed() {
    let f = tempdir().unwrap();
    let summary = OperationEngine::default()
        .spawn_copy(
            vec![vp(f.path().join("missing"))],
            vp(f.path()),
            ScanOptions::default(),
            opts(ConflictPolicy::Skip),
        )
        .join();
    assert_eq!(summary.state, JobState::Failed);
    assert!(!summary.outcome.errors.is_empty());
}

#[test]
fn alias_to_a_descendant_cannot_bypass_ancestry_checks() {
    let f = tempdir().unwrap();
    let source = f.path().join("folder");
    let nested = source.join("nested");
    let alias = f.path().join("alias");
    fs::create_dir_all(&nested).unwrap();
    LocalFs.create_symlink(&vp(&nested), &vp(&alias)).unwrap();
    let control = JobControl::new();
    let plan = scan_sources(
        &LocalFs,
        &[vp(&source)],
        ScanOptions::default(),
        &control,
        |_| {},
    )
    .unwrap();
    let result = copy_plan(
        &LocalFs,
        &plan,
        &vp(&alias),
        opts(ConflictPolicy::Overwrite),
        &control,
        |_| {},
        |_| {},
        reject_ask,
    )
    .unwrap();
    assert!(!result.errors.is_empty());
    assert!(!nested.join("folder").exists());
}

#[test]
fn interrupted_directory_replacement_preserves_the_original_file() {
    for cancel in [true, false] {
        let f = tempdir().unwrap();
        let source = f.path().join("folder");
        let dest = f.path().join("dest");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(source.join("child"), b"child").unwrap();
        fs::write(dest.join("folder"), b"original file").unwrap();
        let control = JobControl::new();
        let plan = scan_sources(
            &LocalFs,
            &[vp(&source)],
            ScanOptions::default(),
            &control,
            |_| {},
        )
        .unwrap();
        let result = copy_plan(
            &LocalFs,
            &plan,
            &vp(&dest),
            opts(ConflictPolicy::Ask),
            &control,
            |_| {},
            |_| {},
            |_| {
                if cancel {
                    control.cancel();
                } else {
                    fs::remove_file(source.join("child")).unwrap();
                }
                Ok(ConflictDecision {
                    action: ConflictAction::Overwrite,
                    apply_to_all: false,
                })
            },
        );
        if cancel {
            assert!(result.is_err());
        } else {
            assert!(!result.unwrap().errors.is_empty());
        }
        assert_eq!(fs::read(dest.join("folder")).unwrap(), b"original file");
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 1);
    }
}

#[test]
fn move_cleanup_retains_children_created_after_scan() {
    let f = tempdir().unwrap();
    let source = f.path().join("folder");
    let dest = f.path().join("dest");
    fs::create_dir(&source).unwrap();
    fs::create_dir_all(dest.join("folder")).unwrap();
    fs::write(source.join("old"), b"source").unwrap();
    fs::write(dest.join("folder/old"), b"destination").unwrap();
    let result = move_sources(
        &LocalFs,
        &[vp(&source)],
        &vp(&dest),
        ScanOptions::default(),
        opts(ConflictPolicy::Ask),
        &JobControl::new(),
        |_| {},
        |_| {},
        |_| {
            fs::write(source.join("new"), b"new during move").unwrap();
            Ok(ConflictDecision {
                action: ConflictAction::Overwrite,
                apply_to_all: false,
            })
        },
    )
    .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(fs::read(source.join("new")).unwrap(), b"new during move");
    assert!(!source.join("old").exists());
}

#[test]
fn no_replace_rename_preserves_files_and_dangling_links() {
    let f = tempdir().unwrap();
    let source = f.path().join("source");
    let target = f.path().join("target");
    fs::write(&source, b"source").unwrap();
    fs::write(&target, b"target").unwrap();
    assert!(
        LocalFs
            .rename_noreplace(&vp(&source), &vp(&target))
            .is_err()
    );
    assert_eq!(fs::read(&target).unwrap(), b"target");
    fs::remove_file(&target).unwrap();
    LocalFs
        .create_symlink(&vp(f.path().join("absent")), &vp(&target))
        .unwrap();
    assert!(
        LocalFs
            .rename_noreplace(&vp(&source), &vp(&target))
            .is_err()
    );
    assert!(fs::symlink_metadata(&target).unwrap().is_symlink());
    assert_eq!(fs::read(&source).unwrap(), b"source");
}

#[test]
fn apply_all_is_job_wide_for_moves() {
    let f = tempfile::tempdir().unwrap();
    let d = f.path().join("dest");
    std::fs::create_dir(&d).unwrap();
    let mut sources = Vec::new();
    for name in ["a", "b"] {
        let p = f.path().join(name);
        std::fs::write(&p, b"source").unwrap();
        std::fs::write(d.join(name), b"old").unwrap();
        sources.push(VPath::from(p));
    }
    let mut prompts = 0;
    let result = move_sources(
        &LocalFs,
        &sources,
        &VPath::from(d),
        ScanOptions::default(),
        TransferOptions::default(),
        &JobControl::new(),
        |_| {},
        |_| {},
        |_| {
            prompts += 1;
            Ok(ConflictDecision {
                action: ConflictAction::Skip,
                apply_to_all: true,
            })
        },
    )
    .unwrap();
    assert!(result.errors.is_empty());
    assert_eq!(prompts, 1);
}
