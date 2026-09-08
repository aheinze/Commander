use super::*;
use relm4::{Component, ComponentController};
use std::path::Path;

fn normalized(uri: &str) -> String {
    RemoteConnection::parse(uri).unwrap().uri
}

#[test]
fn mounted_connections_match_normalized_addresses_and_path_boundaries() {
    for (root, saved, expected) in [
        (
            "sftp://alex@SERVER.invalid/",
            "sftp://alex@server.invalid:22/home/alex",
            Some("home/alex"),
        ),
        (
            "sftp://alex@server.invalid",
            "sftp://alex@server.invalid/",
            Some(""),
        ),
        (
            "smb://nas.invalid/Team",
            "smb://nas.invalid/Team/Design%20files",
            Some("Design files"),
        ),
        ("smb://nas.invalid/Team", "smb://nas.invalid/Teamwork", None),
        (
            "sftp://alex@server.invalid",
            "sftp://sam@server.invalid/home",
            None,
        ),
        (
            "sftp://alex@server.invalid",
            "sftp://alex@server.invalid:2222/home",
            None,
        ),
        (
            "sftp://alex@server.invalid",
            "ftp://alex@server.invalid/home",
            None,
        ),
        (
            "sftp://alex@server.invalid",
            "sftp://alex@other.invalid/home",
            None,
        ),
    ] {
        assert_eq!(
            relative_mount_path(root, saved),
            expected.map(std::path::PathBuf::from),
            "{root} / {saved}"
        );
    }
}

#[test]
fn edits_replace_in_place_deduplicate_and_never_persist_passwords() {
    let original = normalized("sftp://alex@old.example:2222/Work");
    let other = normalized("smb://nas.example/Team");
    let updated = normalized("sftp://sam@new.example:2200/Projects");
    let mut locations = vec![original.clone(), other.clone(), updated.clone()];
    let mut names = BTreeMap::from([(original.clone(), "Work server".to_owned())]);
    save_location(
        &mut locations,
        &mut names,
        "sftp://sam:secret@new.example:2200/Projects",
        Some(&original),
        None,
    )
    .unwrap();
    assert_eq!(locations, vec![updated.clone(), other]);
    assert!(!format!("{locations:?}").contains("secret"));
    assert_eq!(
        names,
        BTreeMap::from([(updated.clone(), "Work server".to_owned())])
    );
    save_location(&mut locations, &mut names, &updated, None, None).unwrap();
    assert_eq!(locations.len(), 2);
    let before = locations.clone();
    let names_before = names.clone();
    assert!(
        save_location(
            &mut locations,
            &mut names,
            "not a server",
            Some(&updated),
            Some("Bad edit")
        )
        .is_err()
    );
    assert!(
        save_location(
            &mut locations,
            &mut names,
            &updated,
            Some(&original),
            Some("Stale edit")
        )
        .is_err()
    );
    assert_eq!(locations, before);
    assert_eq!(names, names_before);
    save_location(
        &mut locations,
        &mut names,
        &updated,
        Some(&updated),
        Some("  Design · Archive  "),
    )
    .unwrap();
    assert_eq!(names.get(&updated).unwrap(), "Design · Archive");
    save_location(&mut locations, &mut names, &updated, None, None).unwrap();
    assert_eq!(names.get(&updated).unwrap(), "Design · Archive");
    save_location(
        &mut locations,
        &mut names,
        &updated,
        Some(&updated),
        Some("   "),
    )
    .unwrap();
    assert!(names.is_empty());
}

#[track_caller]
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Remote editor timed out at {}",
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

fn button(root: &impl IsA<gtk::Widget>, label: &str) -> gtk::Button {
    descendants::<gtk::Button>(root)
        .into_iter()
        .find(|button| {
            button.label().as_deref() == Some(label)
                || button.tooltip_text().as_deref() == Some(label)
        })
        .unwrap()
}

fn entry(root: &impl IsA<gtk::Widget>, value: &str) -> gtk::Entry {
    descendants::<gtk::Entry>(root)
        .into_iter()
        .find(|entry| entry.text() == value)
        .unwrap()
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

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_saved_remotes_edit_from_sidebar_save_offline_cancel_and_keep_credentials_private() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let original = normalized("sftp://alex@old.example:2222/Work");
    let other = normalized("smb://nas.example/Team");
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
                remote_uris: vec![original.clone(), other.clone()],
                remote_names: BTreeMap::from([(original.clone(), "Work server".to_owned())]),
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
    wait_until(|| {
        !app.model().pane(PaneId::Left).loading && !app.widgets().rendered_remotes.is_empty()
    });
    button(&app.widgets().sidebar_remotes, "Edit connection…").emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    assert_eq!(dialog.title(), "Edit Remote Storage");
    entry(&dialog, "Work server").set_text("Cancelled name");
    assert_eq!(entry(&dialog, "old.example").text(), "old.example");
    assert_eq!(entry(&dialog, "2222").text(), "2222");
    assert_eq!(entry(&dialog, "alex").text(), "alex");
    let folder = entry(&dialog, "/Work");
    let password = descendants::<gtk::PasswordEntry>(&dialog).pop().unwrap();
    assert!(password.text().is_empty());
    assert!(button(&dialog, "Save").is_sensitive());
    password.set_text("do-not-persist-this-password");
    assert!(!button(&dialog, "Save").is_sensitive());
    assert!(button(&dialog, "Save and connect").is_sensitive());
    password.set_text("");
    folder.set_text("/Cancelled edit");
    button(&dialog, "Cancel").emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_none());
    assert_eq!(
        app.model().remote_uris,
        vec![original.clone(), other.clone()]
    );
    assert!(password.text().is_empty());

    assert_eq!(
        app.model().remote_names.get(&original).unwrap(),
        "Work server"
    );
    button(&app.widgets().sidebar_remotes, "Edit connection…").emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    entry(&dialog, "Work server").set_text("Design · Archive");
    button(&dialog, "Save").emit_clicked();
    wait_until(|| {
        app.widget().visible_dialog().is_none()
            && app
                .model()
                .remote_names
                .get(&original)
                .is_some_and(|name| name == "Design · Archive")
    });
    assert!(
        descendants::<gtk::Label>(&app.widgets().sidebar_remotes)
            .iter()
            .any(|label| label.text() == "Design · Archive")
    );

    let connect = descendants::<gtk::Button>(&app.widgets().sidebar_remotes)
        .into_iter()
        .find(|button| button.tooltip_text().as_deref() == Some(&original))
        .unwrap();
    let controllers = connect.observe_controllers();
    let mut handled = false;
    for index in 0..controllers.n_items() {
        if let Some(keys) = controllers
            .item(index)
            .and_downcast::<gtk::EventControllerKey>()
        {
            handled |= keys.emit_by_name::<bool>(
                "key-pressed",
                &[&gdk::Key::F10, &0_u32, &gdk::ModifierType::SHIFT_MASK],
            );
        }
    }
    assert!(handled);
    let menu = descendants::<gtk::Popover>(&connect).pop().unwrap();
    wait_until(|| menu.is_visible());
    assert!(menu.has_css_class("file-context-menu"));
    apply_appearance(AppearanceMode::Dark);
    snapshot(app.widget(), "remote-saved-menu-dark");
    // Popovers use a separate native surface, outside the window snapshot.
    if let Some(directory) = std::env::var_os("COMMANDER_TEST_ARTIFACTS") {
        let snapshot = gtk::Snapshot::new();
        menu.snapshot_child(&menu.first_child().unwrap(), &snapshot);
        menu.renderer()
            .unwrap()
            .render_texture(snapshot.to_node().unwrap(), None)
            .save_to_png(Path::new(&directory).join("snapshots/remote-saved-menu-content-dark.png"))
            .unwrap();
    }
    button(&menu, "Edit connection…").emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    assert_eq!(
        entry(&dialog, "Design · Archive").text(),
        "Design · Archive"
    );
    entry(&dialog, "old.example").set_text("new.example");
    entry(&dialog, "2222").set_text("2200");
    entry(&dialog, "alex").set_text("sam");
    entry(&dialog, "/Work").set_text("/Project files");
    snapshot(app.widget(), "remote-edit-dark");
    app.widget().set_default_size(760, 740);
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "remote-edit-narrow-light");
    button(&dialog, "Save").emit_clicked();
    let updated = normalized("sftp://sam@new.example:2200/Project%20files");
    wait_until(|| app.model().remote_uris[0] == updated && app.widget().visible_dialog().is_none());
    assert_eq!(app.model().remote_uris, vec![updated.clone(), other]);
    assert_eq!(
        app.model().remote_names,
        BTreeMap::from([(updated.clone(), "Design · Archive".to_owned())])
    );
    assert!(
        descendants::<gtk::Label>(&app.widgets().sidebar_remotes)
            .iter()
            .any(|label| label.text() == "Design · Archive")
    );
    app.emit(AppMsg::SetPaletteQuery("Design · Archive".to_owned()));
    wait_until(|| app.model().palette_query == "Design · Archive");
    assert!(
        app.model()
            .palette_items()
            .iter()
            .any(|item| item.label == "Connect remote · Design · Archive"
                && matches!(&item.action, PaletteAction::Remote(uri) if uri == &updated))
    );
    assert_eq!(
        app.model().pane(PaneId::Left).current_directory(),
        &VPath::from(root.path())
    );
    assert!(app.model().pane(PaneId::Left).error.is_none());
    gtk::prelude::GtkWindowExt::set_focus(app.widget(), None::<&gtk::Widget>);
    snapshot(app.widget(), "remote-saved-updated-light");
    let remote = button(&app.widgets().sidebar_remotes, &updated);
    let row = remote.parent().unwrap();
    row.set_state_flags(gtk::StateFlags::PRELIGHT, false);
    snapshot(app.widget(), "remote-saved-hover-light");
    row.unset_state_flags(gtk::StateFlags::PRELIGHT);
    remote.grab_focus();
    snapshot(app.widget(), "remote-saved-focus-light");
    let saved_path =
        Path::new(&std::env::var_os("XDG_STATE_HOME").unwrap()).join("dualpane/session.toml");
    wait_until(|| std::fs::read_to_string(&saved_path).is_ok_and(|text| text.contains(&updated)));
    let saved = std::fs::read_to_string(saved_path).unwrap();
    assert!(!saved.contains(&original));
    assert!(!saved.contains("do-not-persist-this-password"));
    let restored: SessionState = toml_edit::de::from_str(&saved).unwrap();
    assert_eq!(
        restored.remote_names.get(&updated).unwrap(),
        "Design · Archive"
    );
    button(&app.widgets().sidebar_remotes, "Edit connection…").emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    entry(&dialog, "Design · Archive").set_text("");
    button(&dialog, "Save").emit_clicked();
    wait_until(|| app.model().remote_names.is_empty() && app.widget().visible_dialog().is_none());
    let endpoint = updated.split_once("://").unwrap().1;
    assert!(
        descendants::<gtk::Label>(&app.widgets().sidebar_remotes)
            .iter()
            .any(|label| label.text() == endpoint)
    );
    app.widget().close();
}
