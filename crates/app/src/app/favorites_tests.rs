use super::*;
use relm4::{Component, ComponentController};

#[test]
#[ignore = "requires an isolated GTK session; run alone with --ignored --test-threads=1"]
fn gtk_favorite_labels_rename_without_changing_destinations() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let original = fixture.path().join("Original folder");
    let other = fixture.path().join("Other folder");
    std::fs::create_dir(&original).unwrap();
    std::fs::create_dir(&other).unwrap();
    let path = VPath::from(original.as_path());
    let path_string = path.to_string();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let session = SessionState {
        bookmarks: vec![path_string.clone(), other.display().to_string()],
        favorite_groups: vec![FavoriteGroupSession {
            name: "Projects".to_owned(),
            paths: vec![path_string.clone()],
            labels: BTreeMap::new(),
        }],
        dual_pane: false,
        sidebar_visible: true,
        preview_visible: false,
        ..SessionState::default()
    };
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
        app.widgets().sidebar_bookmark_rows.len() == 3 && !app.model().pane(PaneId::Left).loading
    });

    let (dialog, entry) = open_rename(&app, 0);
    assert_eq!(entry.text(), "Original folder");
    entry.set_text("Cancelled label");
    response(&dialog, "Cancel");
    wait_until(|| app.widget().visible_dialog().is_none());
    assert!(app.model().bookmark_labels.is_empty());
    // Cancelling must leave the menu usable for another rename.
    let (dialog, entry) = open_rename(&app, 0);
    entry.set_text("  Work / API  ");
    snapshot_dialog(app.widget());
    response(&dialog, "Rename");
    wait_until(|| favorite_label(&app, 0) == "Work / API");
    assert_eq!(app.model().bookmarks[0], path);
    assert_eq!(favorite_label(&app, 2), "Original folder");
    assert_eq!(
        app.widgets().sidebar_bookmark_rows[0]
            .1
            .tooltip_text()
            .as_deref(),
        Some(path_string.as_str())
    );

    app.emit(AppMsg::SetPaletteQuery("Work / API".to_owned()));
    wait_until(|| app.model().palette_query == "Work / API");
    assert!(app.model().palette_items().iter().any(|item| {
        matches!(&item.action, PaletteAction::Navigate(destination) if destination == &path)
    }));
    app.emit(AppMsg::MoveBookmarkToEnd(0));
    wait_until(|| favorite_label(&app, 1) == "Work / API");
    assert_eq!(app.model().bookmarks[1], path);
    app.widgets().sidebar_bookmark_rows[1].1.emit_clicked();
    wait_until(|| {
        app.model().pane(PaneId::Left).current_directory() == &path
            && !app.model().pane(PaneId::Left).loading
    });
    assert!(original.is_dir());
    assert!(!fixture.path().join("Work").exists());

    let (dialog, entry) = open_rename(&app, 2);
    entry.set_text("Client resources");
    response(&dialog, "Rename");
    wait_until(|| favorite_label(&app, 2) == "Client resources");
    assert_eq!(favorite_label(&app, 1), "Work / API");
    assert_eq!(
        app.model().favorite_groups[0].paths.as_slice(),
        std::slice::from_ref(&path_string)
    );

    let project = directories::ProjectDirs::from("org", "example", "Dualpane").unwrap();
    let saved_path = project.state_dir().unwrap().join("session.toml");
    wait_until(|| {
        std::fs::read_to_string(&saved_path)
            .ok()
            .and_then(|text| toml_edit::de::from_str::<SessionState>(&text).ok())
            .is_some_and(|saved| {
                saved.bookmark_labels.get(&path_string).map(String::as_str) == Some("Work / API")
                    && saved.favorite_groups[0]
                        .labels
                        .get(&path_string)
                        .map(String::as_str)
                        == Some("Client resources")
            })
    });
    let (dialog, entry) = open_rename(&app, 1);
    assert_eq!(entry.text(), "Work / API");
    entry.set_text("");
    response(&dialog, "Rename");
    wait_until(|| favorite_label(&app, 1) == "Original folder");
    assert!(app.model().bookmark_labels.is_empty());
    app.emit(AppMsg::RemoveGroupedFavorite { group: 0, item: 0 });
    wait_until(|| app.model().favorite_groups[0].paths.is_empty());
    assert!(app.model().favorite_groups[0].labels.is_empty());
    app.widget().close();
}

fn favorite_label(app: &relm4::Controller<AppModel>, index: usize) -> String {
    app.widgets().sidebar_bookmark_rows[index]
        .1
        .child()
        .unwrap()
        .last_child()
        .and_downcast::<gtk::Label>()
        .unwrap()
        .text()
        .to_string()
}

fn find<T: IsA<gtk::Widget> + glib::object::IsClass>(widget: &gtk::Widget) -> Option<T> {
    if let Ok(found) = widget.clone().downcast::<T>() {
        return Some(found);
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if let Some(found) = find(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

fn open_rename(app: &relm4::Controller<AppModel>, index: usize) -> (adw::Dialog, gtk::Entry) {
    wait_until(|| app.widget().visible_dialog().is_none());
    let button = app.widgets().sidebar_bookmark_rows[index].1.clone();
    let controllers = button.observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(click) = controllers.item(index).and_downcast::<gtk::GestureClick>()
            && click.button() == 3
        {
            click.emit_by_name::<()>("pressed", &[&1_i32, &20.0_f64, &12.0_f64]);
        }
    }
    let menu = find::<gtk::Popover>(button.upcast_ref()).unwrap();
    assert!(menu.is_visible());
    find::<gtk::Button>(&menu.child().unwrap())
        .unwrap()
        .emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    let entry = find::<gtk::Entry>(dialog.upcast_ref()).unwrap();
    (dialog, entry)
}

fn response(dialog: &adw::Dialog, label: &str) {
    fn button(widget: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(button) = widget.downcast_ref::<gtk::Button>()
            && button.label().as_deref() == Some(label)
        {
            return Some(button.clone());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if let Some(found) = button(&widget, label) {
                return Some(found);
            }
            child = widget.next_sibling();
        }
        None
    }
    button(dialog.upcast_ref(), label).unwrap().emit_clicked();
}

fn snapshot_dialog(window: &adw::ApplicationWindow) {
    let Some(directory) = std::env::var_os("COMMANDER_MILLER_SNAPSHOT_DIR") else {
        return;
    };
    let deadline = Instant::now() + Duration::from_millis(300);
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
    let child = gtk::prelude::GtkWindowExt::child(window).unwrap();
    let snapshot = gtk::Snapshot::new();
    child.parent().unwrap().snapshot_child(&child, &snapshot);
    let node = snapshot.to_node().unwrap();
    window
        .renderer()
        .unwrap()
        .render_texture(&node, None)
        .save_to_png(std::path::Path::new(&directory).join("rename-favorite.png"))
        .unwrap();
}

#[track_caller]
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let context = glib::MainContext::default();
    while !ready() {
        assert!(Instant::now() < deadline, "favorite update timed out");
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}
