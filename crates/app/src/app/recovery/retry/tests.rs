use super::*;
use crate::app::ux_tests::{button, launch, wait};
use dualpane_engine::journal::{JobJournal, JournalEvent};
use relm4::ComponentController;
use std::fs;

fn saved_copy(root: &std::path::Path) -> (PathBuf, PathBuf, VPath) {
    let destination = root.join("destination");
    fs::create_dir(&destination).unwrap();
    let completed = root.join("completed");
    let remaining = root.join("remaining");
    fs::write(&completed, b"already transferred").unwrap();
    fs::write(&remaining, b"still to transfer").unwrap();
    let first = OperationEngine::default()
        .spawn_copy(
            vec![completed.clone().into()],
            destination.clone().into(),
            ScanOptions::default(),
            TransferOptions::default(),
        )
        .join();
    assert_eq!(first.state, JobState::Done);
    let journal = JobJournal::create(
        &journal_directory(),
        JobKind::Copy,
        vec![completed.into(), remaining.into()],
        Some(destination.clone().into()),
    )
    .unwrap();
    for record in first.outcome.transfers {
        journal.transfer(&record).unwrap();
    }
    journal
        .append(&JournalEvent::Finished {
            state: JobState::Failed,
            errors: vec!["Connection lost".into()],
            completed_items: 1,
        })
        .unwrap();
    (
        journal.path().to_owned(),
        destination.clone(),
        destination.join("completed").into(),
    )
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_recovery_retry_reviews_original_paths_after_restart_and_reuses_completed_files() {
    let fixture = tempfile::tempdir().unwrap();
    let (path, destination, copied) = saved_copy(fixture.path());
    let identity = LocalFs.stat(&copied, false).unwrap().identity;
    let app = launch(fixture.path());
    wait(|| {
        app.model()
            .recovery_records
            .iter()
            .any(|record| record.path == path)
    });
    assert!(app.model().operations.is_empty());
    app.emit(AppMsg::ShowRecovery);
    wait(|| app.widget().visible_dialog().is_some());
    let list = app.widget().visible_dialog().unwrap();
    let row = format!("Copy · Failed · 1 item\n{}", destination.display());
    button(&list, &row).unwrap().emit_clicked();
    wait(|| {
        button(
            &app.widget().visible_dialog().unwrap(),
            "Retry remaining items…",
        )
        .is_some()
    });
    button(
        &app.widget().visible_dialog().unwrap(),
        "Retry remaining items…",
    )
    .unwrap()
    .emit_clicked();
    wait(|| app.model().recovery_retry.prepared.is_some());
    let review = app.widget().visible_dialog().unwrap();
    button(&review, "Cancel").unwrap().emit_clicked();
    wait(|| app.model().recovery_retry.pending.is_none());
    assert!(!destination.join("remaining").exists());
    assert!(app.model().operations.is_empty());
    list.close();
    wait(|| app.widget().visible_dialog().is_none());
    app.emit(AppMsg::PrepareRecoveryRetry(path.clone()));
    app.emit(AppMsg::PrepareRecoveryRetry(path.clone()));
    wait(|| app.model().recovery_retry.prepared.is_some());
    app.state().get_mut().model.active_pane = PaneId::Right;
    button(
        &app.widget().visible_dialog().unwrap(),
        "Retry remaining items",
    )
    .unwrap()
    .emit_clicked();
    wait(|| app.model().operations.len() == 1 && app.model().active_operations == 0);
    let state = app.model();
    let job = state.operations.values().next().unwrap();
    assert_eq!(job.state, JobState::Done, "{:?}", job.error);
    assert!(matches!(
        job.retry,
        OperationRetry::Copy {
            pane: PaneId::Left,
            ..
        }
    ));
    drop(state);
    assert_eq!(
        fs::read(destination.join("remaining")).unwrap(),
        b"still to transfer"
    );
    assert_eq!(LocalFs.stat(&copied, false).unwrap().identity, identity);
    assert!(read_journal(&path).unwrap().retry.is_some());
    app.widget().close();
    wait(|| !app.widget().is_visible());
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_recovery_retry_rejects_a_changed_record_after_review() {
    let fixture = tempfile::tempdir().unwrap();
    let (path, destination, _) = saved_copy(fixture.path());
    let app = launch(fixture.path());
    wait(|| {
        app.model()
            .recovery_records
            .iter()
            .any(|record| record.path == path)
    });
    app.emit(AppMsg::PrepareRecoveryRetry(path.clone()));
    wait(|| app.model().recovery_retry.prepared.is_some());
    dualpane_engine::journal::mark_reviewed(&path).unwrap();
    button(
        &app.widget().visible_dialog().unwrap(),
        "Retry remaining items",
    )
    .unwrap()
    .emit_clicked();
    wait(|| app.model().recovery_retry.pending.is_none());
    assert!(app.model().operations.is_empty());
    assert!(!destination.join("remaining").exists());
    assert!(read_journal(&path).unwrap().retry.is_none());
    app.widget().close();
    wait(|| !app.widget().is_visible());
}
