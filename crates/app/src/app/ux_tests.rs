use super::*;
use relm4::{Component, ComponentController};

pub(super) fn launch(path: &std::path::Path) -> relm4::Controller<AppModel> {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(path.into()),
                right: Some(path.into()),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                preview_visible: false,
                ..SessionState::default()
            }),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    wait(|| app.model().panes.iter().all(|pane| !pane.loading));
    app
}
pub(super) fn wait(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "UI condition timed out");
        let context = glib::MainContext::default();
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}
pub(super) fn button(widget: &impl IsA<gtk::Widget>, label: &str) -> Option<gtk::Button> {
    let widget = widget.as_ref();
    if let Some(button) = widget.downcast_ref::<gtk::Button>()
        && button.label().as_deref() == Some(label)
    {
        return Some(button.clone());
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Some(found) = button(&widget, label) {
            return Some(found);
        }
    }
    None
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_transfer_retry_keeps_previous_error_and_only_copies_remaining_files() {
    let fixture = tempfile::tempdir().unwrap();
    let destination = fixture.path().join("destination");
    std::fs::create_dir(&destination).unwrap();
    let completed = fixture.path().join("completed");
    let remaining = fixture.path().join("remaining");
    std::fs::write(&completed, b"already transferred").unwrap();
    std::fs::write(&remaining, b"still to transfer").unwrap();
    let first = OperationEngine::default()
        .spawn_copy(
            vec![completed.clone().into()],
            destination.clone().into(),
            ScanOptions::default(),
            TransferOptions::default(),
        )
        .join();
    assert_eq!(first.state, JobState::Done);
    let copied: VPath = destination.join("completed").into();
    let identity = LocalFs.stat(&copied, false).unwrap().identity;
    let app = launch(fixture.path());
    let previous = first.id;
    app.state().get_mut().model.operations.insert(
        previous,
        OperationStatus {
            recovery: RetryState {
                records: first.outcome.transfers,
                ready: true,
                retried: false,
            },
            phase: JobPhase::Copying,
            kind: OperationKind::Files(JobKind::Copy),
            state: JobState::Failed,
            progress: JobProgress::default(),
            control: JobControl::new(),
            commands: std::sync::mpsc::channel().0,
            retry: OperationRetry::Copy {
                pane: PaneId::Left,
                sources: vec![completed.into(), remaining.into()],
                destination: destination.clone().into(),
            },
            waiting_for_conflict: false,
            error: Some("Server disconnected during the previous attempt".into()),
        },
    );
    app.emit(AppMsg::RetryOperation(previous));
    app.emit(AppMsg::RetryOperation(previous));
    wait(|| app.model().operations.len() == 2 && app.model().active_operations == 0);
    assert_eq!(
        app.model().operations[&previous].error.as_deref(),
        Some("Server disconnected during the previous attempt")
    );
    assert!(app.model().operations[&previous].recovery.retried);
    assert_eq!(
        app.model()
            .operations
            .iter()
            .find(|(id, _)| **id != previous)
            .unwrap()
            .1
            .state,
        JobState::Done
    );
    assert!(
        app.widget().visible_dialog().is_none(),
        "retry should not ask to overwrite verified completed files"
    );
    assert_eq!(LocalFs.stat(&copied, false).unwrap().identity, identity);
    assert_eq!(
        std::fs::read(destination.join("remaining")).unwrap(),
        b"still to transfer"
    );
    app.widget().close();
    wait(|| !app.widget().is_visible());
}
