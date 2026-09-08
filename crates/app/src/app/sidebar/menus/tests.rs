use super::*;
use relm4::{Component, ComponentController};

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_sidebar_context_menus_cover_entries_and_target_the_clicked_folder() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let destination = fixture.path().join("Design resources");
    let recent = fixture.path().join("Recent project");
    std::fs::create_dir(&destination).unwrap();
    std::fs::create_dir(&recent).unwrap();
    std::fs::write(fixture.path().join("z-selected.txt"), "not the menu target").unwrap();
    let path = VPath::from(destination.as_path());
    let session = SessionState {
        bookmarks: vec![path.to_string()],
        favorite_groups: vec![FavoriteGroupSession {
            name: "Projects".into(),
            paths: vec![path.to_string()],
            labels: BTreeMap::new(),
        }],
        recent: vec![recent.display().to_string()],
        remote_uris: vec!["sftp://example.invalid/Projects".into()],
        workspaces: vec![WorkspaceSession {
            name: "Design workspace".into(),
            left: fixture.path().display().to_string(),
            right: path.to_string(),
            left_pane: None,
            right_pane: None,
            active_pane: None,
            dual_pane: None,
            vertical_split: None,
            split_position: None,
            sidebar_visible: None,
            preview_visible: None,
            preview_width: None,
        }],
        dual_pane: false,
        sidebar_visible: true,
        preview_visible: false,
        window_width: 1120,
        window_height: 960,
        appearance: AppearanceMode::Dark,
        ..SessionState::default()
    };
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(fixture.path())),
                right: Some(VPath::from(fixture.path())),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(session),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    wait_until(|| {
        app.widgets().sidebar_bookmark_rows.len() == 2 && !app.model().pane(PaneId::Left).loading
    });
    // Every rendered navigable sidebar row, and every favorite group, exposes the shared menu.
    let rows = descendants(app.widgets().sidebar_revealer.upcast_ref())
        .into_iter()
        .filter(|widget| {
            widget.has_css_class("sidebar-row") || widget.has_css_class("favorite-group-heading")
        })
        .collect::<Vec<_>>();
    assert!(rows.len() >= 7);
    for row in &rows {
        let menu = descendants(row)
            .into_iter()
            .find_map(|widget| widget.downcast::<gtk::Popover>().ok())
            .expect("sidebar item missing its menu");
        assert!(menu.has_css_class("file-context-menu"));
        assert!(!menu.has_arrow());
        assert!(
            descendants(menu.upcast_ref())
                .iter()
                .any(|widget| widget.has_css_class("context-menu-item"))
        );
        assert!(
            controllers::<gtk::GestureClick>(row)
                .iter()
                .any(|click| click.button() == gdk::BUTTON_SECONDARY)
        );
        assert!(!controllers::<gtk::EventControllerKey>(row).is_empty());
    }
    let favorite = app.widgets().sidebar_bookmark_rows[0].1.clone();
    app.emit(AppMsg::MoveCursorTo(CursorTarget::Last, false));
    wait_until(|| {
        app.model()
            .focused_path(PaneId::Left)
            .is_some_and(|path| path.as_path() == fixture.path().join("z-selected.txt"))
    });
    let menu = open(favorite.upcast_ref(), true);
    for label in [
        "Open",
        "Open in new tab",
        "Open in other pane",
        "Open in terminal",
        "Copy path",
        "Rename…",
        "Remove from Favorites",
    ] {
        item(&menu, label);
    }
    snapshot(&menu, "favorite-menu-dark");
    let keys = controllers::<gtk::EventControllerKey>(menu.upcast_ref());
    assert!(keys.iter().any(|keys| keys.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::Down, &0_u32, &gdk::ModifierType::empty()]
    )));
    assert!(item(&menu, "Open in new tab").has_focus());
    item(&menu, "Copy path").emit_clicked();
    let clipboard = app.widget().clipboard();
    wait_until(|| !menu.is_visible());
    let copied = glib::MainContext::default()
        .block_on(clipboard.read_text_future())
        .unwrap()
        .unwrap();
    assert_eq!(copied.as_str(), path.to_string());
    assert_eq!(
        app.model().pane(PaneId::Left).current_directory(),
        &VPath::from(fixture.path())
    );
    // Reopening the same menu must not consume its actions or capture the focused file.
    let menu = open(favorite.upcast_ref(), false);
    item(&menu, "Open in terminal").emit_clicked();
    wait_until(|| {
        app.model()
            .terminal_tabs
            .iter()
            .any(|tab| tab.cwd == path.to_string() && !tab.starting)
    });
    assert_eq!(
        app.model().pane(PaneId::Left).current_directory(),
        &VPath::from(fixture.path())
    );
    app.emit(AppMsg::CloseAllTerminals);
    let menu = open(favorite.upcast_ref(), true);
    item(&menu, "Open in new tab").emit_clicked();
    wait_until(|| {
        app.model().pane(PaneId::Left).current_directory() == &path
            && !app.model().pane(PaneId::Left).loading
    });
    assert_eq!(app.model().pane(PaneId::Left).tabs.len(), 2);
    assert!(app.model().folder_action_target.is_none());
    let menu = open(favorite.upcast_ref(), true);
    item(&menu, "Open in other pane").emit_clicked();
    wait_until(|| {
        app.model().pane(PaneId::Right).current_directory() == &path
            && !app.model().pane(PaneId::Right).loading
    });
    let group = descendants(app.widgets().sidebar_bookmarks.upcast_ref())
        .into_iter()
        .find(|widget| widget.has_css_class("favorite-group-heading"))
        .unwrap();
    let menu = open(&group, true);
    item(&menu, "Add current folder to group");
    item(&menu, "Remove favorite group");
    menu.popdown();
    let workspace = descendants(app.widgets().sidebar_workspaces.upcast_ref())
        .into_iter()
        .find(|widget| widget.has_css_class("sidebar-row"))
        .unwrap();
    let menu = open(&workspace, true);
    item(&menu, "Open Workspace");
    item(&menu, "Rename…");
    item(&menu, "Delete Workspace…");
    apply_appearance(AppearanceMode::Light);
    snapshot(&menu, "workspace-menu-light");
    menu.popdown();
    let recent_row = app
        .widgets()
        .sidebar_recent_rows
        .iter()
        .find(|(path, _)| path.as_path() == recent)
        .unwrap()
        .1
        .clone();
    let menu = open(recent_row.upcast_ref(), true);
    item(&menu, "Add to Favorites").emit_clicked();
    wait_until(|| {
        app.model()
            .bookmarks
            .contains(&VPath::from(recent.as_path()))
    });
    let menu = open(recent_row.upcast_ref(), false);
    item(&menu, "Remove from Recent").emit_clicked();
    wait_until(|| !app.model().recent.contains(&VPath::from(recent.as_path())));
    assert!(recent.is_dir());
    assert!(
        descendants(recent_row.upcast_ref())
            .iter()
            .all(|widget| !widget.is::<gtk::Popover>())
    );
    app.emit(AppMsg::ClearRecent);
    wait_until(|| app.model().recent.is_empty());
    app.widget().destroy();
}

fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut result = vec![root.clone()];
    let mut child = root.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        result.extend(descendants(&widget));
    }
    result
}

fn controllers<T: IsA<glib::Object> + IsA<gtk::EventController> + glib::object::IsClass>(
    widget: &gtk::Widget,
) -> Vec<T> {
    let list = widget.observe_controllers();
    (0..list.n_items())
        .filter_map(|index| list.item(index).and_downcast::<T>())
        .collect()
}

fn open(row: &gtk::Widget, keyboard: bool) -> gtk::Popover {
    if row.has_css_class("favorite-group-heading") {
        row.first_child().unwrap().grab_focus();
    } else {
        row.grab_focus();
    }
    if keyboard {
        let window = row
            .root()
            .unwrap()
            .downcast::<adw::ApplicationWindow>()
            .unwrap();
        // Window capture runs first; it must not turn the sidebar shortcut into the palette.
        for key in [gdk::Key::Menu, gdk::Key::F10] {
            let modifiers = if key == gdk::Key::F10 {
                gdk::ModifierType::SHIFT_MASK
            } else {
                gdk::ModifierType::empty()
            };
            assert!(
                controllers::<gtk::EventControllerKey>(window.upcast_ref())
                    .iter()
                    .all(|keys| !keys
                        .emit_by_name::<bool>("key-pressed", &[&key, &0_u32, &modifiers]))
            );
        }
        assert!(
            controllers::<gtk::EventControllerKey>(row)
                .iter()
                .any(|keys| keys.emit_by_name::<bool>(
                    "key-pressed",
                    &[&gdk::Key::F10, &0_u32, &gdk::ModifierType::SHIFT_MASK]
                ))
        );
    } else {
        controllers::<gtk::GestureClick>(row)
            .into_iter()
            .find(|click| click.button() == gdk::BUTTON_SECONDARY)
            .unwrap()
            .emit_by_name::<()>("pressed", &[&1_i32, &20.0_f64, &12.0_f64]);
    }
    let menu = descendants(row)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Popover>().ok())
        .unwrap();
    wait_until(|| menu.is_visible());
    menu
}

fn item(menu: &gtk::Popover, label: &str) -> gtk::Button {
    descendants(menu.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.tooltip_text().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("missing sidebar action: {label}"))
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !condition() {
        assert!(Instant::now() < deadline, "Sidebar menu timed out");
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(menu: &gtk::Popover, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
    let snapshot = gtk::Snapshot::new();
    menu.snapshot_child(&menu.first_child().unwrap(), &snapshot);
    menu.renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(
            std::path::Path::new(&std::env::var_os("COMMANDER_TEST_ARTIFACTS").unwrap())
                .join("snapshots")
                .join(format!("{name}.png")),
        )
        .unwrap();
}
