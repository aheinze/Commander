use super::*;
use relm4::{Component, ComponentController};

#[track_caller]
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let context = glib::MainContext::default();
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "comparison UI timed out at {}",
            std::panic::Location::caller()
        );
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_compare_reviews_directions_mirror_contents_and_rejects_stale_plans() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("Project");
    let right = fixture.path().join("Backup");
    std::fs::create_dir_all(left.join("new folder")).unwrap();
    std::fs::create_dir_all(right.join("extra folder")).unwrap();
    std::fs::write(left.join("new folder/readme.txt"), "new document").unwrap();
    std::fs::write(right.join("extra folder/keep.txt"), "extra document").unwrap();
    std::fs::write(left.join("notes.txt"), "AAAA").unwrap();
    std::fs::write(right.join("notes.txt"), "BBBB").unwrap();
    for path in [left.join("notes.txt"), right.join("notes.txt")] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
            .unwrap();
    }
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(left.clone().into()),
                right: Some(right.clone().into()),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                sidebar_visible: false,
                preview_visible: false,
                window_width: 1000,
                window_height: 740,
                ..SessionState::default()
            }),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    wait_until(|| !app.model().pane(PaneId::Left).loading);
    let state = show(
        Arc::clone(&app.model().vfs),
        app.model().operation_engine.clone(),
        left.clone().into(),
        right.clone().into(),
        app.sender().clone(),
    )
    .unwrap();
    wait_until(|| !state.busy.get() && state.plan.borrow().is_some());
    assert_eq!(state.store.n_items(), 2);
    assert!(!right.join("new folder").exists());
    assert!(state.status.text().contains("sizes and modification times"));
    state.mirror.set_active(true);
    assert_eq!(state.store.n_items(), 3);
    state.direction.set_selected(1);
    assert!(state.route.text().starts_with("Right → Left"));
    assert!(
        state
            .plan
            .borrow()
            .as_ref()
            .unwrap()
            .actions
            .iter()
            .any(|action| action.kind == SyncActionKind::Trash
                && action.relative_path == std::path::Path::new("new folder"))
    );
    state.direction.set_selected(0);
    state.verify.set_active(true);
    wait_until(|| {
        !state.busy.get()
            && state
                .plan
                .borrow()
                .as_ref()
                .is_some_and(|plan| plan.verified)
    });
    assert_eq!(state.store.n_items(), 4);
    assert!(state.status.text().contains("1 replacement"));
    snapshot(app.widget(), "compare-reviewed-plan");
    std::fs::write(right.join("arrived after review.txt"), "unreviewed").unwrap();
    state.apply.emit_clicked();
    wait_until(|| !state.busy.get() && state.status.text().contains("no changes were applied"));
    assert_eq!(std::fs::read(right.join("notes.txt")).unwrap(), b"BBBB");
    assert!(!right.join("new folder").exists());
    assert!(!state.apply.is_sensitive());
    state.mirror.set_active(false);
    state.refresh.emit_clicked();
    wait_until(|| !state.busy.get() && state.plan.borrow().is_some());
    state.apply.emit_clicked();
    wait_until(|| !state.busy.get() && state.status.text().starts_with("Applied"));
    assert_eq!(std::fs::read(right.join("notes.txt")).unwrap(), b"AAAA");
    assert!(right.join("new folder/readme.txt").exists());
    assert!(right.join("extra folder/keep.txt").exists());
    assert!(right.join("arrived after review.txt").exists());
    assert!(!state.apply.is_sensitive());
    state.refresh.emit_clicked();
    wait_until(|| !state.busy.get() && state.plan.borrow().is_some());
    assert_eq!(
        state.store.n_items(),
        0,
        "recovery backups must not become sync differences"
    );
    app.widget().visible_dialog().unwrap().close();
    wait_until(|| state.closed.get());
    app.widget().close();
}

fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    let Some(directory) = std::env::var_os("COMMANDER_COMPARE_SNAPSHOT_DIR") else {
        return;
    };
    let deadline = Instant::now() + Duration::from_millis(350);
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
    let child = gtk::prelude::GtkWindowExt::child(window).unwrap();
    let snapshot = gtk::Snapshot::new();
    window.snapshot_child(&child, &snapshot);
    window
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}
