use super::*;
use relm4::{Component, ComponentController};

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_fresh_folder_navigation_clears_selection_but_history_restores_it() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let start = VPath::from(fixture.path());
    let folder = start.join_name(OsStr::new("Commander"));
    std::fs::create_dir_all(folder.as_path().join(".cargo")).unwrap();
    std::fs::write(folder.as_path().join("LICENSE"), "MIT License\n").unwrap();
    std::fs::write(folder.as_path().join("README.md"), "# Commander\n").unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(start.clone()),
                right: Some(start.clone()),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                left: PaneSession {
                    folders: BTreeMap::from([(
                        folder.to_string(),
                        FolderViewSession {
                            show_hidden: true,
                            cursor_name: Some("LICENSE".into()),
                            ..FolderViewSession::default()
                        },
                    )]),
                    locations: BTreeMap::from([(
                        folder.to_string(),
                        NavigationSession {
                            selected_names: vec!["LICENSE".into()],
                            ..NavigationSession::default()
                        },
                    )]),
                    ..PaneSession::default()
                },
                bookmarks: vec![folder.to_string()],
                sidebar_visible: true,
                preview_visible: false,
                dual_pane: false,
                window_width: 1040,
                window_height: 760,
                ..SessionState::default()
            }),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    wait(|| ready(&app, &start));

    for mode in [
        PaneViewMode::List,
        PaneViewMode::Grid,
        PaneViewMode::Columns,
    ] {
        app.emit(AppMsg::SetViewMode(mode));
        wait(|| app.model().pane(PaneId::Left).view_mode == mode && ready(&app, &start));
        // Start with LICENSE remembered from a previous visit (including persisted state).
        app.emit(AppMsg::Navigate(PaneId::Left, folder.clone()));
        wait(|| ready(&app, &folder));
        assert_clear(&app, mode);
        if mode == PaneViewMode::List {
            let window = app.widget();
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
                        .join("snapshots/folder-with-no-selection.png"),
                )
                .unwrap();
        }

        for entry in ["folder", "breadcrumb", "sidebar", "new tab"] {
            select_license(&app, &folder);
            app.emit(AppMsg::Navigate(PaneId::Left, start.clone()));
            wait(|| ready(&app, &start));
            match entry {
                "folder" => app.emit(AppMsg::OpenRow(PaneId::Left, 0)),
                "breadcrumb" => app.emit(AppMsg::NavigateExact(PaneId::Left, folder.clone())),
                "sidebar" => app.widgets().sidebar_bookmark_rows[0].1.emit_clicked(),
                _ => app.emit(AppMsg::SidebarLocation {
                    path: folder.clone(),
                    action: sidebar::menus::LocationAction::NewTab,
                }),
            }
            wait(|| ready(&app, &folder));
            assert_clear(&app, mode);
        }

        select_license(&app, &folder);
        app.emit(AppMsg::Navigate(PaneId::Left, start.clone()));
        wait(|| ready(&app, &start));
        assert_clear(&app, mode);
        app.emit(AppMsg::Back(PaneId::Left));
        wait(|| ready(&app, &folder));
        assert_eq!(
            app.model().operation_sources(PaneId::Left),
            vec![folder.join_name(OsStr::new("LICENSE"))]
        );
        assert_eq!(
            app.model().pane(PaneId::Left).selection.len(),
            1,
            "{mode:?}: Back did not restore the selection"
        );
        app.emit(AppMsg::Forward(PaneId::Left));
        wait(|| ready(&app, &start));
        assert_clear(&app, mode);

        // Duplicating the current location into a new tab must start unmarked, too.
        app.emit(AppMsg::Navigate(PaneId::Left, folder.clone()));
        wait(|| ready(&app, &folder));
        select_license(&app, &folder);
        let tabs = app.model().pane(PaneId::Left).tabs.len();
        app.emit(AppMsg::NewTab(PaneId::Left));
        wait(|| app.model().pane(PaneId::Left).tabs.len() == tabs + 1 && ready(&app, &folder));
        assert_clear(&app, mode);
        app.emit(AppMsg::Navigate(PaneId::Left, start.clone()));
        wait(|| ready(&app, &start));
    }
    app.widget().destroy();
}

fn select_license(app: &relm4::Controller<AppModel>, folder: &VPath) {
    let (row, selection, column) = {
        let model = app.model();
        let state = model.pane(PaneId::Left);
        let listing = model.selection_listing(PaneId::Left).unwrap();
        let (row, entry) = listing
            .rows()
            .enumerate()
            .find(|(_, entry)| entry.name() == "LICENSE")
            .unwrap();
        let mut selection = Selection::new();
        selection.select(SelectionKey::for_entry(listing.parent(), entry));
        (
            row as u32,
            selection,
            state.miller_columns.len().saturating_sub(1),
        )
    };
    if app.model().pane(PaneId::Left).view_mode == PaneViewMode::Columns {
        app.emit(AppMsg::MillerSelectionChanged {
            pane: PaneId::Left,
            column,
            path: folder.clone(),
            selection,
            row: Some(row),
        });
    } else {
        app.emit(AppMsg::SelectionChanged(PaneId::Left, selection, Some(row)));
    }
    wait(|| app.model().pane(PaneId::Left).selection.len() == 1);
}

fn ready(app: &relm4::Controller<AppModel>, folder: &VPath) -> bool {
    let model = app.model();
    let pane = model.pane(PaneId::Left);
    pane.current_directory() == folder
        && !pane.loading
        && !pane.filtering
        && model
            .selection_listing(PaneId::Left)
            .is_some_and(|listing| listing.parent() == folder && listing.is_complete())
}

fn assert_clear(app: &relm4::Controller<AppModel>, mode: PaneViewMode) {
    // Let native row binding/focus run as well, so this detects ghost GTK highlights.
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        app.model().pane(PaneId::Left).selection.is_empty(),
        "{mode:?}: fresh navigation restored a selection"
    );
    let first = app
        .model()
        .selection_listing(PaneId::Left)
        .and_then(|listing| {
            listing
                .row(0)
                .map(|entry| listing.parent().join_name(entry.name()))
        });
    assert_eq!(
        app.model().focused_path(PaneId::Left),
        first,
        "{mode:?}: fresh keyboard cursor must start at the first item"
    );
    let widgets = app.widgets();
    let pane = &widgets.panes[0];
    assert!(pane.status.text().contains("0 selected"));
    if mode == PaneViewMode::Columns {
        for model in pane
            .miller_columns
            .iter()
            .filter_map(|column| column.model.as_ref())
        {
            assert!(model.selection().is_empty());
        }
    } else {
        assert!(pane.model.selection().is_empty());
    }
}

#[track_caller]
fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !condition() {
        assert!(Instant::now() < deadline, "navigation timed out");
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}
