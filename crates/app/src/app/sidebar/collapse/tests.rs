use super::*;
use relm4::{Component, ComponentController};

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !ready() {
        assert!(Instant::now() < deadline, "Sidebar group check timed out");
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(app: &relm4::Controller<AppModel>, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
    let Some(directory) = std::env::var_os("COMMANDER_TEST_ARTIFACTS") else {
        return;
    };
    let widgets = app.widgets();
    let sidebar = &widgets.sidebar_revealer;
    let snapshot = gtk::Snapshot::new();
    sidebar.parent().unwrap().snapshot_child(sidebar, &snapshot);
    app.widget()
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
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_sidebar_groups_collapse_restore_and_keep_actions_available() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("Design resources");
    std::fs::create_dir(&project).unwrap();
    let saved = BTreeSet::from([
        "devices".to_owned(),
        "remotes".to_owned(),
        favorite_group_key("Projects"),
    ]);
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
                sidebar_visible: true,
                dual_pane: false,
                preview_visible: false,
                window_width: 1040,
                window_height: 860,
                collapsed_sidebar_groups: saved.clone(),
                favorite_groups: vec![FavoriteGroupSession {
                    name: "Projects".into(),
                    paths: vec![project.display().to_string()],
                    labels: BTreeMap::new(),
                }],
                remote_uris: vec!["sftp://example.invalid/Work".into()],
                remote_names: BTreeMap::from([(
                    "sftp://example.invalid/Work".into(),
                    "Work server".into(),
                )]),
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
        !app.model().pane(PaneId::Left).loading && app.widgets().sidebar_favorite_groups.len() == 1
    });
    assert_eq!(app.widgets().sidebar_groups.len(), 6);
    for group in app
        .widgets()
        .sidebar_groups
        .iter()
        .chain(&app.widgets().sidebar_favorite_groups)
    {
        assert_eq!(group.toggle.is_active(), !saved.contains(&group.key));
        assert_eq!(group.revealer.reveals_child(), !saved.contains(&group.key));
    }
    let favorites = app
        .widgets()
        .sidebar_groups
        .iter()
        .find(|group| group.key == "favorites")
        .unwrap()
        .toggle
        .clone();
    favorites.emit_clicked();
    wait_until(|| app.model().collapsed_sidebar_groups.contains("favorites"));
    favorites.grab_focus();
    let window_keys = app.widget().observe_controllers();
    for index in 0..window_keys.n_items() {
        if let Some(keys) = window_keys
            .item(index)
            .and_downcast::<gtk::EventControllerKey>()
        {
            for key in [
                gdk::Key::Left,
                gdk::Key::Right,
                gdk::Key::KP_Left,
                gdk::Key::KP_Right,
                gdk::Key::space,
                gdk::Key::Return,
            ] {
                assert!(
                    !keys.emit_by_name::<bool>(
                        "key-pressed",
                        &[&key, &0_u32, &gdk::ModifierType::empty()]
                    ),
                    "The window must leave header keys to the group"
                );
            }
        }
    }
    let mut controllers = Vec::new();
    let model = favorites.observe_controllers();
    for index in 0..model.n_items() {
        if let Some(keys) = model.item(index).and_downcast::<gtk::EventControllerKey>() {
            controllers.push(keys);
        }
    }
    assert!(controllers.iter().any(|keys| keys.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::Right, &0_u32, &gdk::ModifierType::empty()]
    )));
    wait_until(|| !app.model().collapsed_sidebar_groups.contains("favorites"));
    // Rebuilding groups for a renamed favorite must not expand the saved group.
    app.emit(AppMsg::RenameFavorite {
        group: Some(0),
        path: VPath::from(project.as_path()),
        name: "Design library".into(),
    });
    wait_until(|| {
        app.model().favorite_groups[0]
            .labels
            .values()
            .any(|value| value == "Design library")
    });
    assert!(!app.widgets().sidebar_favorite_groups[0].toggle.is_active());
    app.emit(AppMsg::DevicesChanged);
    wait_until(|| app.widgets().rendered_remote_devices == Some(app.model().devices.revision()));
    assert!(
        !app.widgets()
            .sidebar_groups
            .iter()
            .find(|group| group.key == "remotes")
            .unwrap()
            .revealer
            .reveals_child()
    );
    // The group's independent add control reveals the new item.
    let add = app.widgets().sidebar_favorite_groups[0]
        .heading
        .first_child()
        .unwrap()
        .next_sibling()
        .and_downcast::<gtk::Button>()
        .unwrap();
    add.emit_clicked();
    wait_until(|| {
        app.model().favorite_groups[0].paths.len() == 2
            && app.widgets().sidebar_favorite_groups[0].toggle.is_active()
    });
    apply_appearance(AppearanceMode::Dark);
    snapshot(&app, "sidebar-groups-mixed-dark");
    let toggles: Vec<_> = app
        .widgets()
        .sidebar_groups
        .iter()
        .map(|group| group.toggle.clone())
        .collect();
    for toggle in toggles {
        toggle.set_active(false);
    }
    wait_until(|| {
        app.widgets()
            .sidebar_groups
            .iter()
            .all(|group| app.model().collapsed_sidebar_groups.contains(&group.key))
    });
    wait_until(|| {
        app.widgets()
            .sidebar_groups
            .iter()
            .all(|group| !group.revealer.is_child_revealed())
    });
    assert!(
        app.widgets().sidebar_places.last().unwrap().1.is_mapped(),
        "Trash stays reachable"
    );
    apply_appearance(AppearanceMode::Light);
    snapshot(&app, "sidebar-groups-collapsed-light");
    let state_file = std::path::PathBuf::from(std::env::var_os("XDG_STATE_HOME").unwrap())
        .join("dualpane/session.toml");
    wait_until(|| {
        std::fs::read_to_string(&state_file)
            .ok()
            .and_then(|text| toml_edit::de::from_str::<SessionState>(&text).ok())
            .is_some_and(|saved| {
                saved.collapsed_sidebar_groups == app.model().collapsed_sidebar_groups
            })
    });
    // Adding through a collapsed section header expands only that section.
    let add = app
        .widgets()
        .sidebar_groups
        .iter()
        .find(|group| group.key == "favorites")
        .unwrap()
        .heading
        .last_child()
        .and_downcast::<gtk::Button>()
        .unwrap();
    add.emit_clicked();
    wait_until(|| !app.model().collapsed_sidebar_groups.contains("favorites"));
    assert!(app.model().collapsed_sidebar_groups.contains("remotes"));
    app.widget().close();
}
