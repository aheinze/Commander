use super::*;
use relm4::{Component, ComponentController};

#[test]
fn action_validation_preserves_commands_and_normalizes_file_types() {
    let mut tool = blank_tool();
    assert!(validate(&tool)[0].is_some());
    assert!(validate(&tool)[1].is_some());
    tool.name = "View PDF".to_owned();
    tool.command = "viewer \"%path%\"".to_owned();
    tool.extensions = ".PDF, *.png; jpg PDF".to_owned();
    assert_eq!(validate(&tool), [None; 3]);
    assert_eq!(extensions(&tool.extensions).unwrap(), "pdf, png, jpg");
    tool.command = "viewer \"%path%".to_owned();
    assert!(validate(&tool)[1].unwrap().contains("quote"));
    tool.command = "\"\" %path%".to_owned();
    assert!(validate(&tool)[1].is_some());
    tool.command = "sh -c 'printf hello | cat'".to_owned();
    assert!(
        validate(&tool)[1].is_none(),
        "A command containing pipes must round-trip intact"
    );
    tool.extensions = "*.tar.gz".to_owned();
    assert!(validate(&tool)[2].is_some());
    tool.applies_to = "folders".to_owned();
    assert!(validate(&tool)[2].is_none());
    assert_eq!(
        command_example("viewer %paths%", false).unwrap(),
        "viewer /home/you/Documents/Report.pdf /home/you/Documents/Notes.pdf"
    );
    assert_eq!(
        shlex::split(&command_example("viewer --file=%path% %name% %parent%", false).unwrap())
            .unwrap(),
        [
            "viewer",
            "--file=/home/you/Documents/Report.pdf",
            "Report.pdf",
            "/home/you/Documents"
        ]
    );
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_custom_tools_add_edit_toggle_undo_and_reopen() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(fixture.path())),
                right: Some(VPath::from(fixture.path())),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                sidebar_visible: false,
                preview_visible: false,
                dual_pane: false,
                window_width: 1024,
                window_height: 850,
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
    let (dialog, manager) = build(Vec::new(), app.sender().clone());
    dialog.present(Some(app.widget()));
    wait_until(|| dialog.is_mapped());
    assert!(manager.empty.is_visible());
    snapshot(&dialog, "tools-empty-dark");
    manager.add_empty.emit_clicked();
    for widget in [
        manager.name.entry.upcast_ref::<gtk::Widget>(),
        manager.save.upcast_ref(),
    ] {
        widget.grab_focus();
        for key in [gdk::Key::Escape, gdk::Key::Return, gdk::Key::space] {
            let controllers = app.widget().observe_controllers();
            for index in 0..controllers.n_items() {
                if let Some(controller) = controllers
                    .item(index)
                    .and_downcast::<gtk::EventControllerKey>()
                {
                    assert!(
                        !controller.emit_by_name::<bool>(
                            "key-pressed",
                            &[&key, &0_u32, &gdk::ModifierType::empty()]
                        ),
                        "File-pane shortcuts must not intercept action-manager keys"
                    );
                }
            }
        }
    }
    manager.save.emit_clicked();
    assert!(!manager.name.error.text().is_empty() && !manager.command.error.text().is_empty());
    assert!(app.model().custom_tools.is_empty());
    manager.name.entry.set_text("Open in image editor");
    manager.command.entry.set_text("gimp");
    manager.command.entry.set_position(-1);
    insert_placeholder(&manager.command.entry, "%paths%");
    assert_eq!(manager.command.entry.text(), "gimp %paths%");
    manager.target.set_selected(1);
    manager.extensions.entry.set_text("*.png, .JPG, webp");
    let preview = find_expander(dialog.upcast_ref(), "Preview with example items").unwrap();
    preview.set_expanded(true);
    snapshot(&dialog, "tools-editor-dark");
    apply_appearance(AppearanceMode::Light);
    snapshot(&dialog, "tools-editor-light");
    apply_appearance(AppearanceMode::Dark);
    manager.save.emit_clicked();
    wait_until(|| app.model().custom_tools.len() == 1);
    assert_eq!(app.model().custom_tools[0].extensions, "png, jpg, webp");
    assert_eq!(manager.stack.visible_child_name().as_deref(), Some("list"));

    let switch = find::<gtk::Switch>(manager.list.upcast_ref()).unwrap();
    switch.set_active(false);
    wait_until(|| !app.model().custom_tools[0].enabled);
    button_with_tip(&manager.list, "Edit Open in image editor").emit_clicked();
    assert!(!manager.enabled.is_active());
    manager.name.entry.set_text("Open images");
    manager.command.entry.set_text("gimp \"%paths%");
    manager.save.emit_clicked();
    assert!(!manager.command.error.text().is_empty());
    assert_eq!(app.model().custom_tools[0].name, "Open in image editor");
    snapshot(&dialog, "tools-validation");
    manager.command.entry.set_text("gimp %paths%");
    manager.save.emit_clicked();
    wait_until(|| app.model().custom_tools[0].name == "Open images");
    assert!(!app.model().custom_tools[0].enabled);

    manager.edit(None);
    manager.name.entry.set_text("Open folder in terminal");
    manager
        .command
        .entry
        .set_text("terminal --working-directory=%path%");
    manager.target.set_selected(2);
    assert!(!manager.extensions.root.is_visible());
    manager.save.emit_clicked();
    wait_until(|| app.model().custom_tools.len() == 2);
    snapshot(&dialog, "tools-list-dark");
    apply_appearance(AppearanceMode::Light);
    snapshot(&dialog, "tools-list-light");
    apply_appearance(AppearanceMode::Dark);
    button_with_tip(&manager.list, "Remove Open images").emit_clicked();
    wait_until(|| app.model().custom_tools.len() == 1);
    let toast = manager.undo_notice.borrow().clone().unwrap();
    assert_eq!(toast.button_label().as_deref(), Some("Undo"));
    toast.emit_by_name::<()>("button-clicked", &[]);
    wait_until(|| app.model().custom_tools.len() == 2);
    assert_eq!(app.model().custom_tools[0].name, "Open images");
    assert!(!app.model().custom_tools[0].enabled);

    // Cancel/close must protect a draft, without running or publishing the command.
    manager.edit(Some(0));
    manager.command.entry.set_text("different-app %path%");
    dialog.close();
    wait_until(|| {
        app.widget()
            .visible_dialog()
            .is_some_and(|visible| visible != dialog)
    });
    let confirm = app.widget().visible_dialog().unwrap();
    find_button(confirm.upcast_ref(), "Keep editing")
        .unwrap()
        .emit_clicked();
    wait_until(|| app.widget().visible_dialog().as_ref() == Some(&dialog));
    assert_eq!(manager.command.entry.text(), "different-app %path%");
    manager.leave_editor(false);
    wait_until(|| {
        app.widget()
            .visible_dialog()
            .is_some_and(|visible| visible != dialog)
    });
    let confirm = app.widget().visible_dialog().unwrap();
    find_button(confirm.upcast_ref(), "Discard changes")
        .unwrap()
        .emit_clicked();
    wait_until(|| manager.stack.visible_child_name().as_deref() == Some("list"));
    assert_eq!(app.model().custom_tools[0].command, "gimp %paths%");

    // A compact sheet keeps editor fields and the action footer reachable.
    dialog.set_content_width(420);
    dialog.set_content_height(450);
    manager.edit(Some(1));
    assert!(
        manager
            .example
            .text()
            .contains("/home/you/Documents/Project")
    );
    snapshot(&dialog, "tools-compact");
    assert!(dialog.width() <= app.widget().width());
    assert!(dialog.child().unwrap().width() <= 460);
    assert!(manager.save.is_mapped());
    manager.show_list();
    dialog.close();
    wait_until(|| app.widget().visible_dialog().is_none());

    // Opening again uses live model state, not the settings dialog's original snapshot.
    app.emit(AppMsg::ManageCustomTools);
    wait_until(|| app.widget().visible_dialog().is_some());
    let reopened = app.widget().visible_dialog().unwrap();
    assert!(find_label(reopened.upcast_ref(), "Open images"));
    assert!(find_label(reopened.upcast_ref(), "Open folder in terminal"));
    let state = SessionState {
        custom_tools: app.model().custom_tools.clone(),
        ..SessionState::default()
    };
    let serialized = toml_edit::ser::to_string(&state).unwrap();
    let restored: SessionState = toml_edit::de::from_str(&serialized).unwrap();
    assert_eq!(restored.custom_tools, state.custom_tools);
    reopened.close();
    app.widget().close();
}

fn find<T: IsA<gtk::Widget> + glib::object::IsClass>(widget: &gtk::Widget) -> Option<T> {
    if let Ok(found) = widget.clone().downcast::<T>() {
        return Some(found);
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Some(found) = find(&widget) {
            return Some(found);
        }
    }
    None
}

fn visit(widget: &gtk::Widget, matches: &impl Fn(&gtk::Widget) -> bool) -> Option<gtk::Widget> {
    if matches(widget) {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Some(found) = visit(&widget, matches) {
            return Some(found);
        }
    }
    None
}

fn button_with_tip(list: &gtk::ListBox, tip: &str) -> gtk::Button {
    visit(list.upcast_ref(), &|widget| {
        widget.is::<gtk::Button>() && widget.tooltip_text().as_deref() == Some(tip)
    })
    .unwrap()
    .downcast()
    .unwrap()
}

fn find_button(widget: &gtk::Widget, label: &str) -> Option<gtk::Button> {
    visit(widget, &|widget| {
        widget
            .downcast_ref::<gtk::Button>()
            .is_some_and(|button| button.label().as_deref() == Some(label))
    })
    .and_downcast()
}

fn find_expander(widget: &gtk::Widget, label: &str) -> Option<gtk::Expander> {
    visit(widget, &|widget| {
        widget
            .downcast_ref::<gtk::Expander>()
            .is_some_and(|expander| expander.label().as_deref() == Some(label))
    })
    .and_downcast()
}

fn find_label(widget: &gtk::Widget, label: &str) -> bool {
    visit(widget, &|widget| {
        widget
            .downcast_ref::<gtk::Label>()
            .is_some_and(|widget| widget.text() == label)
    })
    .is_some()
}

#[track_caller]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let context = glib::MainContext::default();
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "Tool manager condition timed out"
        );
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(dialog: &adw::Dialog, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(220);
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
    let Some(directory) = std::env::var_os("COMMANDER_TOOLS_SNAPSHOT_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("COMMANDER_TEST_ARTIFACTS")
                .map(|path| std::path::PathBuf::from(path).join("snapshots"))
        })
    else {
        return;
    };
    let child = dialog.first_child().unwrap();
    let snapshot = gtk::Snapshot::new();
    child.parent().unwrap().snapshot_child(&child, &snapshot);
    let node = snapshot.to_node().unwrap();
    let renderer = dialog
        .root()
        .and_downcast::<gtk::Window>()
        .unwrap()
        .renderer()
        .unwrap();
    renderer
        .render_texture(&node, None)
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}
