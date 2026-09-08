use super::*;
use relm4::{Component, ComponentController};
use std::path::Path;

#[track_caller]
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Secure delete timed out at {}",
            std::panic::Location::caller()
        );
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn descendants<T: IsA<gtk::Widget> + glib::object::IsClass>(
    root: &impl IsA<gtk::Widget>,
) -> Vec<T> {
    let mut found = Vec::new();
    if let Ok(widget) = root.as_ref().clone().downcast::<T>() {
        found.push(widget);
    }
    let mut child = root.as_ref().first_child();
    while let Some(widget) = child {
        found.extend(descendants::<T>(&widget));
        child = widget.next_sibling();
    }
    found
}

fn button(dialog: &adw::Dialog, name: &str) -> gtk::Button {
    descendants::<gtk::Button>(dialog)
        .into_iter()
        .find(|button| button.label().as_deref() == Some(name))
        .unwrap()
}

fn ready(dialog: &adw::Dialog) -> bool {
    descendants::<gtk::Label>(dialog)
        .iter()
        .any(|label| label.text().contains("ready to overwrite"))
}

fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
    let Some(directory) = std::env::var_os("COMMANDER_TEST_ARTIFACTS") else {
        return;
    };
    let child = gtk::prelude::GtkWindowExt::child(window).unwrap();
    let snapshot = gtk::Snapshot::new();
    window.snapshot_child(&child, &snapshot);
    window
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(
            Path::new(&directory)
                .join("snapshots")
                .join(format!("{name}.png")),
        )
        .unwrap();
}

fn open_review(app: &relm4::Controller<AppModel>, path: &Path, kind: EntryKind) -> adw::Dialog {
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((VPath::from(path), kind)),
    ));
    app.emit(AppMsg::ExecuteCommand(CommandId::SecureDelete));
    wait_until(|| app.widget().visible_dialog().is_some());
    app.widget().visible_dialog().unwrap()
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_secure_delete_reviews_cancels_captures_selection_and_rejects_stale_files() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("Private notes.txt");
    let keep = root.path().join("Keep this file.txt");
    let folder = root.path().join("Folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(&private, vec![42; 500_000]).unwrap();
    std::fs::write(&keep, "unchanged").unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(root.path())),
                right: Some(VPath::from(root.path())),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                dual_pane: false,
                sidebar_visible: false,
                preview_visible: false,
                window_width: 1080,
                window_height: 800,
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
    let dialog = open_review(&app, &private, EntryKind::File);
    wait_until(|| ready(&dialog));
    let erase = button(&dialog, "Overwrite and delete");
    assert!(!erase.is_sensitive());
    assert!(app.model().operations.is_empty());
    assert_eq!(std::fs::metadata(&private).unwrap().len(), 500_000);
    apply_appearance(AppearanceMode::Dark);
    snapshot(app.widget(), "secure-delete-review-dark");
    app.widget().set_default_size(720, 740);
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "secure-delete-review-narrow-light");
    button(&dialog, "Cancel").emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_none());
    assert!(app.model().operations.is_empty());
    assert_eq!(std::fs::read(&private).unwrap(), vec![42; 500_000]);

    let dialog = open_review(&app, &private, EntryKind::File);
    wait_until(|| ready(&dialog));
    // A later selection change cannot redirect the reviewed destructive operation.
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((VPath::from(keep.as_path()), EntryKind::File)),
    ));
    wait_until(|| {
        app.model()
            .focused_item(PaneId::Left)
            .is_some_and(|(path, _)| path == VPath::from(keep.as_path()))
    });
    descendants::<gtk::CheckButton>(&dialog)[0].set_active(true);
    assert!(button(&dialog, "Overwrite and delete").is_sensitive());
    snapshot(app.widget(), "secure-delete-ready-light");
    apply_appearance(AppearanceMode::Dark);
    snapshot(app.widget(), "secure-delete-ready-dark");
    button(&dialog, "Overwrite and delete").set_state_flags(gtk::StateFlags::PRELIGHT, false);
    snapshot(app.widget(), "secure-delete-ready-hover-dark");
    button(&dialog, "Overwrite and delete").unset_state_flags(gtk::StateFlags::PRELIGHT);
    apply_appearance(AppearanceMode::Light);
    button(&dialog, "Overwrite and delete").emit_clicked();
    wait_until(|| !private.exists() && app.model().active_operations == 0);
    assert_eq!(std::fs::read_to_string(&keep).unwrap(), "unchanged");
    assert!(
        app.model()
            .operations
            .values()
            .any(|operation| operation.kind == OperationKind::SecureDelete
                && operation.state == JobState::Done)
    );
    assert!(app.model().undo_stack.is_empty());
    wait_until(|| {
        app.widget().visible_dialog().is_none() && !app.model().pane(PaneId::Left).loading
    });
    snapshot(app.widget(), "secure-delete-completed-light");

    std::fs::write(&private, "reviewed contents").unwrap();
    app.emit(AppMsg::Refresh(PaneId::Left));
    wait_until(|| {
        let model = app.model();
        let pane = model.pane(PaneId::Left);
        !pane.loading
            && pane.active().listing.as_ref().is_some_and(|listing| {
                listing
                    .rows()
                    .any(|entry| entry.name() == "Private notes.txt")
            })
    });
    let dialog = open_review(&app, &private, EntryKind::File);
    wait_until(|| ready(&dialog));
    std::fs::write(&private, "new contents must be kept").unwrap();
    descendants::<gtk::CheckButton>(&dialog)[0].set_active(true);
    button(&dialog, "Overwrite and delete").emit_clicked();
    wait_until(|| {
        app.model().operations.values().any(|operation| {
            operation.kind == OperationKind::SecureDelete && operation.state == JobState::Failed
        })
    });
    assert_eq!(
        std::fs::read_to_string(&private).unwrap(),
        "new contents must be kept"
    );
    wait_until(|| app.widget().visible_dialog().is_none() && app.model().active_operations == 0);
    let dialog = open_review(&app, &folder, EntryKind::Directory);
    wait_until(|| {
        descendants::<gtk::Label>(&dialog)
            .iter()
            .any(|label| label.text().contains("Cannot securely delete"))
    });
    descendants::<gtk::CheckButton>(&dialog)[0].set_active(true);
    assert!(!button(&dialog, "Overwrite and delete").is_sensitive());
    snapshot(app.widget(), "secure-delete-unsupported-light");
    dialog.close();
    app.widget().close();
}
