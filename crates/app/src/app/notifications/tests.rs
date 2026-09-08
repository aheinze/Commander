use super::*;
use relm4::{Component, ComponentController};

#[track_caller]
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Toast check timed out at {}",
            std::panic::Location::caller()
        );
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn pending() -> Vec<adw::Toast> {
    BUS.with_borrow(|bus| {
        bus.pending
            .iter()
            .filter_map(glib::WeakRef::upgrade)
            .collect()
    })
}

fn clear() {
    let toasts = pending();
    BUS.with_borrow_mut(|bus| *bus = Bus::default());
    for toast in toasts {
        toast.dismiss();
    }
}

fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(350);
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
            std::path::Path::new(&directory)
                .join("snapshots")
                .join(format!("{name}.png")),
        )
        .unwrap();
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_toasts_replace_inline_feedback_queue_safely_and_work_in_dialogs() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(directory.path())),
                right: Some(VPath::from(directory.path())),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                dual_pane: false,
                sidebar_visible: true,
                preview_visible: false,
                window_width: 1120,
                window_height: 860,
                ..SessionState::default()
            }),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    wait_until(|| !app.model().pane(PaneId::Left).loading && app.widget().is_mapped());
    clear();
    let message = "Cannot open <draft> & notes: permission denied.";
    app.emit(AppMsg::OpenFailed(PaneId::Left, message.into()));
    wait_until(|| test_messages().iter().any(|text| text == message));
    assert!(
        !app.widgets().panes[0]
            .status
            .text()
            .contains("permission denied")
    );
    let first = pending()[0].clone();
    assert!(!first.uses_markup());
    for value in ["a", "b", "c"] {
        app.emit(AppMsg::SetPaletteQuery(value.into()));
    }
    wait_until(|| app.model().palette_query == "c");
    assert_eq!(pending(), [first]);
    apply_appearance(AppearanceMode::Dark);
    snapshot(app.widget(), "toast-error-dark");
    app.emit(AppMsg::Refresh(PaneId::Left));
    wait_until(|| {
        app.model().pane(PaneId::Left).error.is_none() && !app.model().pane(PaneId::Left).loading
    });
    clear();
    let long = format!(
        "Could not save the connection. {}",
        "A very long server address with Unicode: 設計/資料/überblick. ".repeat(8)
    );
    error(&long);
    let toast = pending()[0].clone();
    assert_eq!(toast.button_label().as_deref(), Some("Copy"));
    toast.emit_by_name::<()>("button-clicked", &[]);
    let copied = Rc::new(RefCell::new(None));
    let result = copied.clone();
    glib::spawn_future_local(async move {
        *result.borrow_mut() = Some(
            gdk::Display::default()
                .unwrap()
                .clipboard()
                .read_text_future()
                .await
                .unwrap()
                .unwrap()
                .to_string(),
        );
    });
    wait_until(|| copied.borrow().is_some());
    assert_eq!(copied.borrow().as_deref(), Some(long.trim()));
    app.widget().set_default_size(760, 740);
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "toast-long-compact-light");
    clear();
    for index in 0..20 {
        info(&format!("Queued message {index}"));
    }
    assert_eq!(pending().len(), 4);
    clear();
    error("An error must survive informational traffic");
    let protected = pending()[0].clone();
    for index in 0..8 {
        info(&format!("Background update {index}"));
    }
    assert_eq!(pending().len(), 4);
    assert!(pending().contains(&protected));
    clear();
    for index in 0..4 {
        error(&format!("Important failure {index}"));
    }
    let protected = pending();
    info("A lower priority update cannot evict errors");
    assert_eq!(pending(), protected);
    clear();

    let (dialog, view) = dialogs::utility_dialog("Connection details", 480, 300, "remote-dialog");
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Server address"));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.add_css_class("dialog-body");
    body.append(&entry);
    view.set_content(Some(&body));
    dialog.present(Some(app.widget()));
    wait_until(|| entry.is_mapped());
    let feedback = Feedback::default();
    feedback.validate(Some("First invalid value"), &entry);
    feedback.validate(Some("Enter a valid port between 1 and 65535."), &entry);
    wait_until(|| !pending().is_empty());
    assert_eq!(test_messages(), ["Enter a valid port between 1 and 65535."]);
    assert!(dialog.child().unwrap().is::<adw::ToastOverlay>());
    snapshot(app.widget(), "toast-dialog-compact-light");
    pending()[0].dismiss();
    wait_until(|| pending().is_empty());
    assert_eq!(
        entry.tooltip_text().as_deref(),
        Some("Enter a valid port between 1 and 65535.")
    );
    feedback.validate(None, &entry);
    assert!(entry.tooltip_text().is_none());
    // Wide text can truncate below a character-count threshold in this sheet.
    let diagnostic = "診断資料の保存に失敗しました".repeat(10);
    assert!(diagnostic.chars().count() < 160);
    error(&diagnostic);
    let toast = pending()[0].clone();
    assert_eq!(toast.button_label().as_deref(), Some("Copy"));
    let title = toast
        .custom_title()
        .unwrap()
        .last_child()
        .and_downcast::<gtk::Label>()
        .unwrap();
    wait_until(|| title.is_mapped() && title.layout().is_ellipsized());
    clear();
    feedback.validate(
        Some("A closing dialog must not emit late validation"),
        &entry,
    );
    dialog.close();
    wait_until(|| app.widget().visible_dialog().is_none());
    drop(feedback);
    // A new sheet must receive its own feedback, even inside the dedup window.
    let repeat = "This message belongs to each newly opened dialog";
    error(repeat);
    let (second, _) = dialogs::utility_dialog("Another connection", 480, 300, "remote-dialog");
    second.present(Some(app.widget()));
    wait_until(|| second.is_mapped());
    error(repeat);
    assert_eq!(
        test_messages()
            .iter()
            .filter(|text| text.as_str() == repeat)
            .count(),
        2
    );
    second.close();
    wait_until(|| app.widget().visible_dialog().is_none());
    clear();
    info("Connection saved");
    pending()[0].set_timeout(1);
    wait_until(|| pending().is_empty());
    assert!(test_messages().is_empty());
    app.emit(AppMsg::RemoteConnected {
        uri: "sftp://test.invalid/projects".into(),
        result: Err("Permission denied".into()),
    });
    wait_until(|| !pending().is_empty());
    assert!(app.widget().visible_dialog().is_none());
    let failure = pending()[0].clone();
    assert_eq!(failure.button_label().as_deref(), Some("Options"));
    failure.emit_by_name::<()>("button-clicked", &[]);
    wait_until(|| app.widget().visible_dialog().is_some());
    app.widget().visible_dialog().unwrap().close();
    wait_until(|| app.widget().visible_dialog().is_none());
    clear();
    dialogs::show_checksum_result(
        &VPath::from(directory.path()),
        Err("Unreadable file".into()),
    );
    assert!(app.widget().visible_dialog().is_none());
    assert_eq!(test_messages(), ["Checksum failed: Unreadable file"]);
    app.widget().close();
}
