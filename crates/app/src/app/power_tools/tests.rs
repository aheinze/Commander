use super::*;
use relm4::{Component, ComponentController};

fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let context = glib::MainContext::default();
    while !ready() {
        assert!(Instant::now() < deadline, "Power tool UI timed out");
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}
fn children(root: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut result = vec![root.as_ref().clone()];
    let mut child = root.as_ref().first_child();
    while let Some(widget) = child {
        result.extend(children(&widget));
        child = widget.next_sibling();
    }
    result
}
fn button(dialog: &adw::Dialog, text: &str) -> gtk::Button {
    children(dialog)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.label().as_deref() == Some(text))
        .unwrap_or_else(|| panic!("Missing button {text}"))
}
fn entry(dialog: &adw::Dialog, prefix: &str) -> gtk::Entry {
    children(dialog)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Entry>().ok())
        .find(|e| e.placeholder_text().is_some_and(|s| s.starts_with(prefix)))
        .unwrap()
}
fn text(dialog: &adw::Dialog, part: &str) -> bool {
    children(dialog)
        .iter()
        .filter_map(|w| w.downcast_ref::<gtk::Label>())
        .any(|l| l.text().contains(part))
}
fn table(dialog: &adw::Dialog) -> gtk::ColumnView {
    children(dialog)
        .into_iter()
        .find_map(|w| w.downcast::<gtk::ColumnView>().ok())
        .unwrap()
}
fn capture(window: &adw::ApplicationWindow, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(250);
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
    let snapshot = gtk::Snapshot::new();
    window.snapshot_child(
        &gtk::prelude::GtkWindowExt::child(window).unwrap(),
        &snapshot,
    );
    window
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(
            std::path::Path::new(&std::env::var_os("COMMANDER_TEST_ARTIFACTS").unwrap())
                .join(format!("snapshots/{name}.png")),
        )
        .unwrap();
}
fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    let dialog = window.visible_dialog().unwrap();
    let nodes = children(&dialog);
    assert!(dialog.has_css_class("utility-dialog"));
    assert!(nodes.iter().any(
        |widget| widget.has_css_class("window-close") && widget.has_css_class("window-action")
    ));
    let body = nodes
        .iter()
        .find(|widget| widget.has_css_class("dialog-body"))
        .unwrap();
    let footer = nodes
        .iter()
        .find(|widget| widget.has_css_class("dialog-actions"))
        .unwrap();
    assert_eq!(
        body.parent(),
        footer.parent(),
        "Footer must sit outside the inset body"
    );
    assert!(!table(&dialog).shows_column_separators());
    let width = dialog.content_width();
    capture(window, name);
    theme::apply_appearance(AppearanceMode::Light);
    dialog.set_content_width(620);
    capture(window, &format!("{name}-compact-light"));
    for widget in children(&dialog) {
        if widget.is_mapped()
            && (widget.has_css_class("dialog-button") || widget.is::<gtk::Entry>())
        {
            let bounds = widget.compute_bounds(&dialog).unwrap();
            assert!(
                bounds.x() >= 0.0 && bounds.x() + bounds.width() <= dialog.width() as f32 + 1.0,
                "Control overflow in {}",
                dialog.title()
            );
        }
    }
    dialog.set_content_width(width);
    theme::apply_appearance(AppearanceMode::Dark);
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_power_tools_preview_duplicates_and_compare() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("left");
    let right = fixture.path().join("right");
    std::fs::create_dir(&left).unwrap();
    std::fs::create_dir(&right).unwrap();
    std::fs::write(left.join("a.txt"), "first\nlast\n").unwrap();
    std::fs::write(left.join("b.txt"), "first\nlast\n").unwrap();
    std::fs::write(right.join("a.txt"), "first\nnew line\nlast\n").unwrap();
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
                window_width: 1160,
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
    wait(|| !app.model().pane(PaneId::Left).loading && !app.model().pane(PaneId::Right).loading);
    app.emit(AppMsg::ExecuteCommand(CommandId::SelectAll));
    wait(|| app.model().pane(PaneId::Left).selection.len() == 2);
    app.emit(AppMsg::ExecuteCommand(CommandId::BatchRename));
    wait(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    let regex = children(&dialog)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
        .find(|b| b.label().as_deref() == Some("Regular expression"))
        .unwrap();
    regex.set_active(true);
    entry(&dialog, "Text to find").set_text("[");
    assert!(!button(&dialog, "Rename items").is_sensitive());
    assert!(entry(&dialog, "Text to find").has_css_class("error"));
    wait(|| {
        notifications::test_messages()
            .iter()
            .any(|message| message.contains("Invalid regular expression"))
    });
    assert!(
        children(&dialog)
            .iter()
            .filter(|w| w.has_css_class("file-tool-summary"))
            .filter_map(|w| w.downcast_ref::<gtk::Label>())
            .all(|label| !label.text().contains("Invalid regular expression"))
    );
    regex.set_active(false);
    entry(&dialog, "Text to find").set_text("a");
    entry(&dialog, "Replacement").set_text("b");
    assert!(!button(&dialog, "Rename items").is_sensitive());
    assert!(text(&dialog, "Conflicts: 1") || text(&dialog, "Conflicts: 2"));
    entry(&dialog, "Replacement").set_text("renamed");
    assert!(button(&dialog, "Rename items").is_sensitive());
    // Capture the settled preview after the transient validation toast expires.
    wait(|| !text(&dialog, "Invalid regular expression"));
    snapshot(app.widget(), "rename-live-preview");
    button(&dialog, "Rename items").emit_clicked();
    wait(|| left.join("renamed.txt").exists() && app.widget().visible_dialog().is_none());
    app.emit(AppMsg::ExecuteCommand(CommandId::Undo));
    wait(|| left.join("a.txt").exists());

    duplicates(
        Arc::clone(&app.model().vfs),
        VPath::from(left.as_path()),
        app.sender().clone(),
    );
    wait(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    button(&dialog, "Scan folder").emit_clicked();
    wait(|| text(&dialog, "Groups: 1"));
    assert_eq!(table(&dialog).model().unwrap().n_items(), 2);
    snapshot(app.widget(), "duplicate-groups");
    dialog.close();
    wait(|| app.widget().visible_dialog().is_none());

    compare(
        Arc::clone(&app.model().vfs),
        VPath::from(left.join("a.txt")),
        VPath::from(right.join("a.txt")),
    );
    wait(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    wait(|| text(&dialog, "Difference groups: 1"));
    button(&dialog, "Next difference").emit_clicked();
    assert_eq!(table(&dialog).model().unwrap().n_items(), 3);
    snapshot(app.widget(), "file-differences");
    dialog.close();
    wait(|| app.widget().visible_dialog().is_none());

    app.widget().close();
}
