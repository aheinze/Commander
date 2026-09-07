use super::*;
use dualpane_core::{Capabilities, Entry, Metadata};
use dualpane_vfs::{LocalFs, ReadSeek, Result as VfsResult, TrashLocation, VfsError, WriteSeek};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tempfile::tempdir;

fn path(value: impl AsRef<std::path::Path>) -> VPath {
    VPath::from(value.as_ref())
}

struct TestFs {
    trash: PathBuf,
    fail_rename_at: usize,
    rename_count: AtomicUsize,
}
impl TestFs {
    fn new(trash: PathBuf) -> Self {
        Self {
            trash,
            fail_rename_at: 0,
            rename_count: AtomicUsize::new(0),
        }
    }
}
impl Vfs for TestFs {
    fn read_dir(
        &self,
        p: &VPath,
        c: &CancelToken,
    ) -> VfsResult<Box<dyn Iterator<Item = VfsResult<Entry>> + Send>> {
        LocalFs.read_dir(p, c)
    }
    fn stat(&self, p: &VPath, f: bool) -> VfsResult<Metadata> {
        LocalFs.stat(p, f)
    }
    fn open_read(&self, p: &VPath) -> VfsResult<Box<dyn ReadSeek>> {
        LocalFs.open_read(p)
    }
    fn create_write(&self, p: &VPath, h: SizeHint) -> VfsResult<Box<dyn WriteSeek>> {
        LocalFs.create_write(p, h)
    }
    fn open_write(&self, p: &VPath) -> VfsResult<Box<dyn WriteSeek>> {
        LocalFs.open_write(p)
    }
    fn create_dir(&self, p: &VPath) -> VfsResult<()> {
        LocalFs.create_dir(p)
    }
    fn set_len(&self, p: &VPath, n: u64) -> VfsResult<()> {
        LocalFs.set_len(p, n)
    }
    fn preserve_metadata(&self, s: &VPath, d: &VPath, m: &Metadata) -> VfsResult<Vec<String>> {
        LocalFs.preserve_metadata(s, d, m)
    }
    fn sync_file(&self, p: &VPath) -> VfsResult<()> {
        LocalFs.sync_file(p)
    }
    fn rename(&self, s: &VPath, d: &VPath) -> VfsResult<()> {
        LocalFs.rename(s, d)
    }
    fn rename_noreplace(&self, s: &VPath, d: &VPath) -> VfsResult<()> {
        if self.rename_count.fetch_add(1, Ordering::SeqCst) + 1 == self.fail_rename_at {
            return Err(VfsError::Io {
                operation: "injected rename failure",
                path: d.clone(),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            });
        }
        LocalFs.rename_noreplace(s, d)
    }
    fn remove(&self, p: &VPath, k: EntryKind) -> VfsResult<()> {
        LocalFs.remove(p, k)
    }
    fn capabilities(&self) -> Capabilities {
        LocalFs.capabilities()
    }
    fn prepare_trash(&self, _: &VPath) -> VfsResult<TrashLocation> {
        let files = self.trash.join("files");
        let info = self.trash.join("info");
        fs::create_dir_all(&files).unwrap();
        fs::create_dir_all(&info).unwrap();
        Ok(TrashLocation {
            files: path(files),
            info: path(info),
            deletion_date: "2026-09-07T00:00:00".into(),
        })
    }
}

#[test]
fn batch_rename_rejects_collisions_with_unchanged_items_in_either_order() {
    for reverse in [false, true] {
        let f = tempdir().unwrap();
        let a = f.path().join("a.txt");
        let b = f.path().join("b.txt");
        fs::write(&a, b"A").unwrap();
        fs::write(&b, b"B").unwrap();
        let mut items = vec![(path(&a), "b.txt".into()), (path(&b), "b.txt".into())];
        if reverse {
            items.reverse();
        }
        assert!(batch_rename(&LocalFs, &items, &CancelToken::new()).is_err());
        assert_eq!(fs::read(&a).unwrap(), b"A");
        assert_eq!(fs::read(&b).unwrap(), b"B");
    }
}

#[test]
fn overlapping_batch_rename_undo_redo_preserves_both_files() {
    let f = tempdir().unwrap();
    let a = f.path().join("a.txt");
    let aa = f.path().join("aa.txt");
    let aaa = f.path().join("aaa.txt");
    fs::write(&a, b"A").unwrap();
    fs::write(&aa, b"B").unwrap();
    let moves = batch_rename(
        &LocalFs,
        &[(path(&a), "aa.txt".into()), (path(&aa), "aaa.txt".into())],
        &CancelToken::new(),
    )
    .unwrap();
    let mut history = HistoryEntry::BatchRename { moves };
    apply_history(
        OperationEngine::default(),
        &LocalFs,
        &mut history,
        HistoryDirection::Undo,
    )
    .unwrap();
    assert_eq!(fs::read(&a).unwrap(), b"A");
    assert_eq!(fs::read(&aa).unwrap(), b"B");
    apply_history(
        OperationEngine::default(),
        &LocalFs,
        &mut history,
        HistoryDirection::Redo,
    )
    .unwrap();
    assert_eq!(fs::read(&aa).unwrap(), b"A");
    assert_eq!(fs::read(&aaa).unwrap(), b"B");
    assert!(!a.exists());
}

#[test]
fn batch_rename_rolls_back_each_staging_and_publication_failure() {
    for fail_at in 1..=6 {
        let f = tempdir().unwrap();
        let a = f.path().join("a");
        let b = f.path().join("b");
        let c = f.path().join("c");
        fs::write(&a, b"A").unwrap();
        fs::write(&b, b"B").unwrap();
        fs::write(&c, b"C").unwrap();
        let mut vfs = TestFs::new(f.path().join("trash"));
        vfs.fail_rename_at = fail_at;
        let result = rename_paths(
            &vfs,
            &[
                (path(&a), path(&b)),
                (path(&b), path(&c)),
                (path(&c), path(&a)),
            ],
            &CancelToken::new(),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&a).unwrap(), b"A", "failure {fail_at}");
        assert_eq!(fs::read(&b).unwrap(), b"B");
        assert_eq!(fs::read(&c).unwrap(), b"C");
        assert_eq!(fs::read_dir(f.path()).unwrap().count(), 3);
    }
}

#[test]
fn batch_rename_rejects_dot_names_and_observes_cancellation() {
    let f = tempdir().unwrap();
    let a = f.path().join("a");
    fs::write(&a, b"A").unwrap();
    for name in [".", ".."] {
        assert!(batch_rename(&LocalFs, &[(path(&a), name.into())], &CancelToken::new()).is_err());
    }
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(batch_rename(&LocalFs, &[(path(&a), "b".into())], &cancel).is_err());
    assert_eq!(fs::read(&a).unwrap(), b"A");
}

#[test]
fn skipped_copy_has_no_undo_that_could_remove_existing_destination() {
    let f = tempdir().unwrap();
    let source = f.path().join("file");
    let dest = f.path().join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&source, b"source").unwrap();
    fs::write(dest.join("file"), b"existing").unwrap();
    let summary = OperationEngine::default()
        .spawn_copy(
            vec![path(&source)],
            path(&dest),
            ScanOptions::default(),
            TransferOptions {
                conflict_policy: ConflictPolicy::Skip,
                ..TransferOptions::default()
            },
        )
        .join();
    assert!(summary.outcome.errors.is_empty());
    assert!(transfer_history(JobKind::Copy, summary.outcome.transfers).is_none());
    assert_eq!(fs::read(dest.join("file")).unwrap(), b"existing");
}

#[test]
fn copy_keep_both_history_restores_exact_output_on_redo() {
    let f = tempdir().unwrap();
    let vfs = Arc::new(TestFs::new(f.path().join("trash")));
    let engine = OperationEngine::new(vfs.clone());
    let source = f.path().join("file");
    let dest = f.path().join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&source, b"copied").unwrap();
    fs::write(dest.join("file"), b"existing").unwrap();
    let summary = engine
        .spawn_copy(
            vec![path(&source)],
            path(&dest),
            ScanOptions::default(),
            TransferOptions {
                conflict_policy: ConflictPolicy::Rename,
                ..TransferOptions::default()
            },
        )
        .join();
    assert!(
        summary.outcome.errors.is_empty(),
        "{:?}",
        summary.outcome.errors
    );
    let mut history = transfer_history(JobKind::Copy, summary.outcome.transfers).unwrap();
    apply_history(
        engine.clone(),
        vfs.as_ref(),
        &mut history,
        HistoryDirection::Undo,
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("file")).unwrap(), b"existing");
    assert!(!dest.join("file (2)").exists());
    fs::write(&source, b"source edited after undo").unwrap();
    apply_history(
        engine.clone(),
        vfs.as_ref(),
        &mut history,
        HistoryDirection::Redo,
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("file (2)")).unwrap(), b"copied");
    apply_history(engine, vfs.as_ref(), &mut history, HistoryDirection::Undo).unwrap();
    assert_eq!(fs::read(dest.join("file")).unwrap(), b"existing");
}

#[test]
fn undo_refuses_modified_outputs_and_unrecorded_children() {
    for directory in [false, true] {
        let f = tempdir().unwrap();
        let source = f.path().join("source");
        let dest = f.path().join("dest");
        fs::create_dir(&dest).unwrap();
        if directory {
            fs::create_dir(&source).unwrap();
            fs::write(source.join("original"), b"original").unwrap();
        } else {
            fs::write(&source, b"original").unwrap();
        }
        let summary = OperationEngine::default()
            .spawn_copy(
                vec![path(&source)],
                path(&dest),
                ScanOptions::default(),
                TransferOptions::default(),
            )
            .join();
        assert!(summary.outcome.errors.is_empty());
        let mut history = transfer_history(JobKind::Copy, summary.outcome.transfers).unwrap();
        let changed = if directory {
            dest.join("source/new")
        } else {
            dest.join("source")
        };
        fs::write(&changed, b"user edit").unwrap();
        assert!(
            apply_history(
                OperationEngine::default(),
                &LocalFs,
                &mut history,
                HistoryDirection::Undo
            )
            .is_err()
        );
        assert_eq!(fs::read(changed).unwrap(), b"user edit");
    }
}

#[test]
fn copy_redo_refuses_a_new_occupant() {
    let f = tempdir().unwrap();
    let vfs = Arc::new(TestFs::new(f.path().join("trash")));
    let engine = OperationEngine::new(vfs.clone());
    let source = f.path().join("source");
    let dest = f.path().join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&source, b"original").unwrap();
    let summary = engine
        .spawn_copy(
            vec![path(&source)],
            path(&dest),
            ScanOptions::default(),
            TransferOptions::default(),
        )
        .join();
    let mut history = transfer_history(JobKind::Copy, summary.outcome.transfers).unwrap();
    apply_history(
        engine.clone(),
        vfs.as_ref(),
        &mut history,
        HistoryDirection::Undo,
    )
    .unwrap();
    fs::write(dest.join("source"), b"new occupant").unwrap();
    assert!(apply_history(engine, vfs.as_ref(), &mut history, HistoryDirection::Redo).is_err());
    assert_eq!(fs::read(dest.join("source")).unwrap(), b"new occupant");
}

#[test]
fn move_keep_both_undo_redo_uses_exact_names() {
    let f = tempdir().unwrap();
    let source = f.path().join("file");
    let dest = f.path().join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&source, b"moved").unwrap();
    fs::write(dest.join("file"), b"existing").unwrap();
    let engine = OperationEngine::default();
    let summary = engine
        .spawn_move(
            vec![path(&source)],
            path(&dest),
            ScanOptions::default(),
            TransferOptions {
                conflict_policy: ConflictPolicy::Rename,
                ..TransferOptions::default()
            },
        )
        .join();
    assert!(
        summary.outcome.errors.is_empty(),
        "{:?}",
        summary.outcome.errors
    );
    let mut history = transfer_history(JobKind::Move, summary.outcome.transfers).unwrap();
    apply_history(
        engine.clone(),
        &LocalFs,
        &mut history,
        HistoryDirection::Undo,
    )
    .unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"moved");
    assert!(!dest.join("file (2)").exists());
    apply_history(engine, &LocalFs, &mut history, HistoryDirection::Redo).unwrap();
    assert_eq!(fs::read(dest.join("file (2)")).unwrap(), b"moved");
    assert_eq!(fs::read(dest.join("file")).unwrap(), b"existing");
    assert!(!source.exists());
}

#[test]
fn directory_copy_undo_redo_round_trip_preserves_the_tree() {
    let f = tempdir().unwrap();
    let vfs = Arc::new(TestFs::new(f.path().join("trash")));
    let engine = OperationEngine::new(vfs.clone());
    let source = f.path().join("source");
    let dest = f.path().join("dest");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::write(source.join("nested/file"), b"contents").unwrap();
    let summary = engine
        .spawn_copy(
            vec![path(&source)],
            path(&dest),
            ScanOptions::default(),
            TransferOptions::default(),
        )
        .join();
    assert!(
        summary.outcome.errors.is_empty(),
        "{:?}",
        summary.outcome.errors
    );
    let mut history = transfer_history(JobKind::Copy, summary.outcome.transfers).unwrap();
    apply_history(
        engine.clone(),
        vfs.as_ref(),
        &mut history,
        HistoryDirection::Undo,
    )
    .unwrap();
    assert!(!dest.join("source").exists());
    apply_history(engine, vfs.as_ref(), &mut history, HistoryDirection::Redo).unwrap();
    assert_eq!(
        fs::read(dest.join("source/nested/file")).unwrap(),
        b"contents"
    );
}

#[test]
fn replacing_or_merging_existing_items_does_not_offer_destructive_undo() {
    for directory in [true, false] {
        let f = tempdir().unwrap();
        let source = f.path().join("source");
        let dest = f.path().join("dest");
        fs::create_dir(&dest).unwrap();
        if directory {
            fs::create_dir(&source).unwrap();
            fs::write(source.join("new"), b"new").unwrap();
            fs::create_dir(dest.join("source")).unwrap();
            fs::write(dest.join("source/existing"), b"existing").unwrap();
        } else {
            fs::write(&source, b"new").unwrap();
            fs::write(dest.join("source"), b"existing").unwrap();
        }
        let summary = OperationEngine::default()
            .spawn_copy(
                vec![path(&source)],
                path(&dest),
                ScanOptions::default(),
                TransferOptions {
                    conflict_policy: ConflictPolicy::Overwrite,
                    ..TransferOptions::default()
                },
            )
            .join();
        assert!(summary.outcome.errors.is_empty());
        assert!(transfer_history(JobKind::Copy, summary.outcome.transfers).is_none());
        if directory {
            assert_eq!(fs::read(dest.join("source/existing")).unwrap(), b"existing");
        }
    }
}

#[test]
fn journalled_overwrites_survive_restart_and_round_trip_undo_redo() {
    for moving in [false, true] {
        for merged in [false, true] {
            let fixture = tempdir().unwrap();
            let source = fixture
                .path()
                .join(if merged { "tree" } else { "file.txt" });
            let destination = fixture.path().join("destination");
            fs::create_dir(&destination).unwrap();
            let target = destination.join(source.file_name().unwrap());
            let (source_file, target_file) = if merged {
                fs::create_dir(&source).unwrap();
                fs::create_dir(&target).unwrap();
                fs::write(target.join("unrelated.txt"), b"keep me").unwrap();
                (source.join("file.txt"), target.join("file.txt"))
            } else {
                (source.clone(), target.clone())
            };
            fs::write(&source_file, b"new contents").unwrap();
            fs::write(&target_file, b"old contents").unwrap();
            let vfs = Arc::new(TestFs::new(fixture.path().join("trash")));
            let engine = OperationEngine::new(vfs.clone())
                .with_journal_directory(fixture.path().join("jobs"));
            let options = TransferOptions {
                conflict_policy: ConflictPolicy::Overwrite,
                verify: true,
                durable: true,
                parallel: false,
            };
            let handle = if moving {
                engine.spawn_move(
                    vec![path(&source)],
                    path(&destination),
                    ScanOptions::default(),
                    options,
                )
            } else {
                engine.spawn_copy(
                    vec![path(&source)],
                    path(&destination),
                    ScanOptions::default(),
                    options,
                )
            };
            let summary = handle.join();
            assert_eq!(
                summary.state,
                JobState::Done,
                "{:?}",
                summary.outcome.errors
            );
            assert_eq!(fs::read(&target_file).unwrap(), b"new contents");
            assert_eq!(summary.outcome.backups.len(), 1);
            let history = fileops::transfer_history_with_backups(
                summary.kind,
                summary.outcome.transfers,
                summary.outcome.backups,
            )
            .expect("reversible replacement");
            let mut history: HistoryEntry =
                serde_json::from_slice(&serde_json::to_vec(&history).unwrap()).unwrap();
            apply_history(
                engine.clone(),
                vfs.as_ref(),
                &mut history,
                HistoryDirection::Undo,
            )
            .unwrap();
            assert_eq!(fs::read(&target_file).unwrap(), b"old contents");
            assert_eq!(fs::read(&source_file).unwrap(), b"new contents");
            apply_history(engine, vfs.as_ref(), &mut history, HistoryDirection::Redo).unwrap();
            assert_eq!(fs::read(&target_file).unwrap(), b"new contents");
            assert_eq!(source.exists(), !moving);
            if merged {
                assert_eq!(fs::read(target.join("unrelated.txt")).unwrap(), b"keep me");
            }
        }
    }
}

#[test]
fn overwrite_undo_refuses_modified_outputs_and_retains_the_original() {
    let fixture = tempdir().unwrap();
    let source = fixture.path().join("file");
    let destination = fixture.path().join("destination");
    fs::create_dir(&destination).unwrap();
    fs::write(&source, b"new").unwrap();
    fs::write(destination.join("file"), b"old").unwrap();
    let vfs = Arc::new(TestFs::new(fixture.path().join("trash")));
    let engine =
        OperationEngine::new(vfs.clone()).with_journal_directory(fixture.path().join("jobs"));
    let summary = engine
        .spawn_copy(
            vec![path(&source)],
            path(&destination),
            ScanOptions::default(),
            TransferOptions {
                conflict_policy: ConflictPolicy::Overwrite,
                ..TransferOptions::default()
            },
        )
        .join();
    let backup = summary.outcome.backups[0].backup.clone();
    let mut history = fileops::transfer_history_with_backups(
        summary.kind,
        summary.outcome.transfers,
        summary.outcome.backups,
    )
    .unwrap();
    fs::write(destination.join("file"), b"user edits after copying").unwrap();
    assert!(apply_history(engine, vfs.as_ref(), &mut history, HistoryDirection::Undo).is_err());
    assert_eq!(
        fs::read(destination.join("file")).unwrap(),
        b"user edits after copying"
    );
    assert_eq!(fs::read(backup.as_path()).unwrap(), b"old");
}

#[test]
fn recovery_restores_only_missing_originals() {
    let fixture = tempdir().unwrap();
    let source = fixture.path().join("file");
    let destination = fixture.path().join("destination");
    fs::create_dir(&destination).unwrap();
    fs::write(&source, b"new").unwrap();
    fs::write(destination.join("file"), b"old").unwrap();
    let summary = OperationEngine::default()
        .with_journal_directory(fixture.path().join("jobs"))
        .spawn_copy(
            vec![path(&source)],
            path(&destination),
            ScanOptions::default(),
            TransferOptions {
                conflict_policy: ConflictPolicy::Overwrite,
                ..TransferOptions::default()
            },
        )
        .join();
    let record =
        dualpane_engine::journal::read_journal(summary.outcome.journal_path.as_ref().unwrap())
            .unwrap();
    assert!(
        super::recovery::restore_absent_backups(&LocalFs, &record)
            .unwrap()
            .contains("Restored 0")
    );
    assert_eq!(fs::read(destination.join("file")).unwrap(), b"new");
    fs::remove_file(destination.join("file")).unwrap();
    assert!(
        super::recovery::restore_absent_backups(&LocalFs, &record)
            .unwrap()
            .contains("Restored 1")
    );
    assert_eq!(fs::read(destination.join("file")).unwrap(), b"old");
}
