use super::*;
use relm4::{Component, ComponentController};

#[test]
fn startup_preferences_preserve_explicit_paths_and_view_mode() {
    let saved = PaneSession {
        tabs: vec!["/saved/one".into(), "/saved/two".into()],
        active_tab: 1,
        view_mode: PaneViewMode::Columns,
        ..PaneSession::default()
    };
    let fresh = startup_pane(&saved, false, None);
    assert!(fresh.tabs.is_empty());
    assert_eq!(fresh.view_mode, PaneViewMode::Columns);
    let path = VPath::from("/explicit");
    assert_eq!(
        startup_pane(&saved, false, Some(&path)).tabs,
        vec!["/explicit"]
    );
    assert_eq!(startup_pane(&saved, true, None).tabs, saved.tabs);
    let old = toml_edit::ser::to_string(&SessionState::default()).unwrap();
    let mut value: toml_edit::DocumentMut = old.parse().unwrap();
    value.remove("workflow");
    let loaded: SessionState = toml_edit::de::from_str(&value.to_string()).unwrap();
    assert_eq!(loaded.workflow, WorkflowPreferences::default());
}

fn shortcut_reassignment_preserves_alternatives_and_other_profile() {
    let mut overrides = KeymapOverrides::default();
    overrides
        .classic
        .insert("open".into(), vec!["<Control>k".into(), "Return".into()]);
    overrides
        .modern
        .insert("open".into(), vec!["<Alt>o".into()]);
    let keymap = Keymap::new(KeymapProfile::Classic, overrides);
    keymap.assign(CommandId::Rename, Some("<Control>k"));
    assert_eq!(
        keymap.command_for(gdk::Key::k, gdk::ModifierType::CONTROL_MASK),
        Some(CommandId::Rename)
    );
    assert_eq!(keymap.accelerators(CommandId::Open), vec!["Return"]);
    assert_eq!(keymap.overrides().modern["open"], vec!["<Alt>o"]);
    keymap.assign(CommandId::Rename, None);
    assert_eq!(
        keymap.command_for(gdk::Key::k, gdk::ModifierType::CONTROL_MASK),
        None
    );
    let stored = toml_edit::ser::to_string(&keymap.overrides()).unwrap();
    let restored = Keymap::new(
        KeymapProfile::Classic,
        toml_edit::de::from_str(&stored).unwrap(),
    );
    assert!(restored.accelerators(CommandId::Rename).is_empty());
    // Existing event controllers share this Rc and must see configuration changes.
    let shared = restored.clone();
    restored.configure(KeymapProfile::Modern, restored.overrides());
    assert_eq!(
        shared.command_for(gdk::Key::o, gdk::ModifierType::ALT_MASK),
        Some(CommandId::Open)
    );
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_settings_shortcuts_preferences_about_and_persistence() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    shortcut_reassignment_preserves_alternatives_and_other_profile();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("a.txt"), "hello").unwrap();
    std::fs::create_dir(fixture.path().join("z-folder")).unwrap();
    let (worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(fixture.path())),
                right: Some(VPath::from(fixture.path())),
                ..AppOptions::default()
            },
            session_worker: worker,
            session: Some(SessionState {
                sidebar_visible: false,
                preview_visible: false,
                window_width: 1100,
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
    let (cancelled, controls) = build(
        SettingsDraft::from_model(&app.model()),
        app.sender().clone(),
    );
    cancelled.present(Some(app.widget()));
    wait_until(|| cancelled.is_mapped());
    control::<gtk::Switch>(&cancelled, "Keep folders first").set_active(false);
    assert!(app.model().workflow.directories_first);
    controls.cancel.emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_none());
    assert!(app.model().workflow.directories_first);

    let (dialog, ui) = build(
        SettingsDraft::from_model(&app.model()),
        app.sender().clone(),
    );
    dialog.present(Some(app.widget()));
    wait_until(|| dialog.is_mapped());
    snapshot(&dialog, "settings-workflow-dark");
    control::<gtk::Switch>(&dialog, "Restore tabs and folders").set_active(false);
    control::<gtk::Switch>(&dialog, "Keep folders first").set_active(false);
    control::<gtk::Switch>(&dialog, "Browse archives in Commander").set_active(false);
    ui.stack.set_visible_child_name("keyboard");
    ui.search.set_text("rename");
    wait_until(|| ui.shortcuts.first_child().is_some() && ui.search.text() == "rename");
    settle();
    snapshot(&dialog, "settings-keyboard-dark");
    control::<gtk::Button>(&dialog, "Rename").emit_clicked();
    wait_until(|| app.widget().visible_dialog().as_ref() != Some(&dialog));
    let recorder_dialog = app.widget().visible_dialog().unwrap();
    let recorder = find(recorder_dialog.upcast_ref(), &|widget| {
        widget.has_css_class("shortcut-recorder")
    })
    .unwrap();
    recorder.grab_focus();
    // The application's capture controller must defer to the settings UI.
    let controllers = app.widget().observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(keys) = controllers
            .item(index)
            .and_downcast::<gtk::EventControllerKey>()
        {
            assert!(!keys.emit_by_name::<bool>(
                "key-pressed",
                &[&gdk::Key::F5, &0_u32, &gdk::ModifierType::empty()]
            ));
        }
    }
    let keys = recorder.observe_controllers();
    let keys = (0..keys.n_items())
        .find_map(|i| keys.item(i).and_downcast::<gtk::EventControllerKey>())
        .unwrap();
    assert!(keys.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::F5, &0_u32, &gdk::ModifierType::empty()]
    ));
    snapshot(&recorder_dialog, "settings-shortcut-conflict");
    assert!(!keys.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::Tab, &0_u32, &gdk::ModifierType::empty()]
    ));
    assert!(keys.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::Return, &0_u32, &gdk::ModifierType::empty()]
    ));
    wait_until(|| app.widget().visible_dialog().as_ref() == Some(&dialog));
    assert_eq!(
        ui.draft
            .borrow()
            .keymap()
            .command_for(gdk::Key::F5, gdk::ModifierType::empty()),
        Some(CommandId::Rename)
    );
    assert_ne!(
        app.model()
            .keymap
            .command_for(gdk::Key::F5, gdk::ModifierType::empty()),
        Some(CommandId::Rename)
    );
    ui.stack.set_visible_child_name("appearance");
    control::<gtk::Switch>(&dialog, "Calculate folder sizes").set_active(false);
    control::<gtk::Switch>(&dialog, "Show Git information").set_active(false);
    snapshot(&dialog, "settings-appearance-dark");
    ui.stack.set_visible_child_name("privacy");
    control::<gtk::Switch>(&dialog, "Remember recent locations").set_active(false);
    snapshot(&dialog, "settings-history-dark");
    ui.stack.set_visible_child_name("about");
    assert!(
        find(dialog.upcast_ref(), &|widget| widget
            .downcast_ref::<gtk::Label>()
            .is_some_and(
                |l| l.text() == format!("Version {}", env!("CARGO_PKG_VERSION"))
            ))
        .is_some()
    );
    button(&dialog, "Copy details").emit_clicked();
    assert!(button(&dialog, "Copy details").is_sensitive());
    assert!(notifications::test_messages().contains(&"Diagnostics copied".to_owned()));
    let text = glib::MainContext::default()
        .block_on(dialog.clipboard().read_text_future())
        .unwrap()
        .unwrap();
    assert_eq!(text, diagnostics());
    snapshot(&dialog, "settings-about-dark");
    apply_appearance(AppearanceMode::Light);
    snapshot(&dialog, "settings-about-light");
    ui.stack.set_visible_child_name("workflow");
    dialog.set_content_width(620);
    snapshot(&dialog, "settings-workflow-compact-light");
    apply_appearance(AppearanceMode::Dark);
    ui.apply.emit_clicked();
    wait_until(|| {
        app.widget().visible_dialog().is_none()
            && !app.model().workflow.directories_first
            && !app.model().pane(PaneId::Left).loading
    });
    assert!(!app.model().workflow.restore_tabs);
    assert!(!app.model().workflow.browse_archives);
    assert!(!app.model().workflow.inspector_git);
    assert!(!app.model().workflow.inspector_folder_sizes);
    assert!(app.model().recent.is_empty());
    assert_eq!(
        app.model()
            .pane(PaneId::Left)
            .active()
            .listing
            .as_ref()
            .unwrap()
            .row(0)
            .unwrap()
            .name(),
        OsStr::new("a.txt")
    );
    let keymap_path = std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap())
        .join("dualpane/keymaps.toml");
    wait_until(|| keymap_path.exists());
    let stored: KeymapOverrides =
        toml_edit::de::from_str(&std::fs::read_to_string(&keymap_path).unwrap()).unwrap();
    let reloaded = Keymap::new(KeymapProfile::Classic, stored);
    assert_eq!(
        reloaded.command_for(gdk::Key::F5, gdk::ModifierType::empty()),
        Some(CommandId::Rename)
    );
    app.emit(AppMsg::NavigateActive(VPath::from(
        fixture.path().join("z-folder"),
    )));
    wait_until(|| {
        app.model().pane(PaneId::Left).current_directory()
            == &VPath::from(fixture.path().join("z-folder"))
    });
    assert!(
        app.model().recent.is_empty(),
        "Disabled history must remain empty after navigation"
    );
    let (dialog, reopened) = build(
        SettingsDraft::from_model(&app.model()),
        app.sender().clone(),
    );
    assert!(!reopened.draft.borrow().workflow.directories_first);
    assert_eq!(
        reopened
            .draft
            .borrow()
            .keymap()
            .command_for(gdk::Key::F5, gdk::ModifierType::empty()),
        Some(CommandId::Rename)
    );
    dialog.force_close();
    app.widget().close();
}

fn find(widget: &gtk::Widget, matches: &impl Fn(&gtk::Widget) -> bool) -> Option<gtk::Widget> {
    if matches(widget) {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find(&current, matches) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

fn control<T: IsA<gtk::Widget> + glib::types::StaticType>(dialog: &adw::Dialog, title: &str) -> T {
    let label = find(dialog.upcast_ref(), &|widget| {
        widget
            .downcast_ref::<gtk::Label>()
            .is_some_and(|label| label.text() == title)
    })
    .unwrap_or_else(|| panic!("Missing {title}"));
    label
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .last_child()
        .unwrap()
        .downcast::<T>()
        .ok()
        .unwrap()
}

fn button(dialog: &adw::Dialog, text: &str) -> gtk::Button {
    find(dialog.upcast_ref(), &|widget| {
        widget
            .downcast_ref::<gtk::Button>()
            .is_some_and(|button| button.label().as_deref() == Some(text))
    })
    .unwrap()
    .downcast()
    .unwrap()
}

fn settle() {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(250) {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "Settings condition timed out");
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn snapshot(dialog: &adw::Dialog, name: &str) {
    fn redraw(widget: &gtk::Widget) {
        widget.queue_draw();
        let mut child = widget.first_child();
        while let Some(current) = child {
            redraw(&current);
            child = current.next_sibling();
        }
    }
    redraw(dialog.upcast_ref());
    settle();
    let directory = std::path::PathBuf::from(std::env::var_os("COMMANDER_TEST_ARTIFACTS").unwrap())
        .join("snapshots");
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
        .save_to_png(directory.join(format!("{name}.png")))
        .unwrap();
}
