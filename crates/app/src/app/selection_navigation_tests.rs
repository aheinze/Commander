use super::*;
use relm4::ComponentController;

type App = relm4::Controller<AppModel>;

fn settle() {
    let until = Instant::now() + Duration::from_millis(100);
    while Instant::now() < until {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn command(app: &App, command: CommandId) {
    app.emit(AppMsg::ExecuteCommand(command));
    settle();
}

fn model(app: &App) -> ListingListModel {
    let widgets = app.widgets();
    if app.model().pane(PaneId::Left).view_mode == PaneViewMode::Columns {
        widgets.panes[0].miller_columns[0].model.clone().unwrap()
    } else {
        widgets.panes[0].model.clone()
    }
}

fn cursor(app: &App) -> u32 {
    let state = app.model();
    let pane = state.pane(PaneId::Left);
    if pane.view_mode == PaneViewMode::Columns {
        pane.miller_columns[0].selected_row.unwrap()
    } else {
        pane.cursor_row
    }
}

fn path(app: &App, row: u32) -> VPath {
    let listing = model(app).listing().unwrap();
    listing
        .parent()
        .join_name(listing.row(row as usize).unwrap().name())
}

fn click(app: &App, row: u32, modify: bool, extend: bool) {
    {
        let widgets = app.widgets();
        let pane = &widgets.panes[0];
        match app.model().pane(PaneId::Left).view_mode {
            PaneViewMode::List => {
                pane.column_view
                    .scroll_to(row, None, gtk::ListScrollFlags::FOCUS, None)
            }
            PaneViewMode::Grid => pane
                .grid_view
                .scroll_to(row, gtk::ListScrollFlags::FOCUS, None),
            PaneViewMode::Columns => pane.miller_columns[0].view.as_ref().unwrap().scroll_to(
                row,
                gtk::ListScrollFlags::FOCUS,
                None,
            ),
        }
    }
    settle();
    // Exercise GTK's real click/range action, not a synthetic app selection.
    gtk::prelude::GtkWindowExt::focus(app.widget())
        .unwrap()
        .activate_action("listitem.select", Some(&(modify, extend).to_variant()))
        .unwrap();
    settle();
}

#[track_caller]
fn assert_selection(app: &App, rows: &[u32], marked: bool) {
    let expected: Vec<_> = rows.iter().map(|&row| path(app, row)).collect();
    let state = app.model();
    let pane = state.pane(PaneId::Left);
    assert_eq!(
        pane.selection.len(),
        rows.len(),
        "{:?}: selection size",
        pane.view_mode
    );
    assert_eq!(
        pane.selection.is_marked(),
        marked,
        "{:?}: marking intent",
        pane.view_mode
    );
    assert_eq!(
        state.operation_sources(PaneId::Left),
        expected,
        "{:?}: operation targets",
        pane.view_mode
    );
    let selection = model(app).selection();
    assert_eq!(selection.size(), rows.len() as u64, "GTK selection size");
    for &row in rows {
        assert!(selection.contains(row), "GTK must highlight row {row}");
    }
}

#[track_caller]
fn assert_follows(app: &App) {
    let row = cursor(app);
    assert_selection(app, &[row], false);
    assert_eq!(app.model().focused_path(PaneId::Left), Some(path(app, row)));
    let focus = gtk::prelude::GtkWindowExt::focus(app.widget()).unwrap();
    let name = format!(
        "commander-context:{}",
        gio::File::for_path(path(app, row).as_path()).uri()
    );
    assert!(
        contains_widget(&focus, &name),
        "{:?}: GTK focus {focus:?} must be on selected file {name}",
        app.model().pane(PaneId::Left).view_mode
    );
}

fn contains_widget(widget: &gtk::Widget, name: &str) -> bool {
    if widget.widget_name() == name {
        return true;
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if contains_widget(&widget, name) {
            return true;
        }
    }
    false
}

fn snapshot(app: &App, name: &str) {
    // Let the previous row's focus transition fade before capturing the result.
    settle();
    settle();
    let directory = std::path::PathBuf::from(std::env::var_os("COMMANDER_TEST_ARTIFACTS").unwrap());
    let child = gtk::prelude::GtkWindowExt::child(app.widget()).unwrap();
    let snapshot = gtk::Snapshot::new();
    app.widget().snapshot_child(&child, &snapshot);
    app.widget()
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(directory.join(format!("snapshots/{name}.png")))
        .unwrap();
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_selection_follows_navigation_and_preserves_marks_in_all_views() {
    let fixture = tempfile::tempdir().unwrap();
    for index in 0..30 {
        std::fs::write(
            fixture.path().join(format!("file-{index:02}.txt")),
            b"selection fixture",
        )
        .unwrap();
    }
    for name in ["zebra.txt", "zoo.txt"] {
        std::fs::write(fixture.path().join(name), b"type-ahead fixture").unwrap();
    }
    let other = tempfile::tempdir().unwrap();
    let app = ux_tests::launch(fixture.path());
    // Navigation history is keyed by folder. Set up a tab at another folder
    // before marking files so the new-tab entry does not replace that history.
    app.emit(AppMsg::NewTab(PaneId::Left));
    ux_tests::wait(|| {
        app.model().pane(PaneId::Left).active_tab == 1 && !app.model().pane(PaneId::Left).loading
    });
    app.emit(AppMsg::Navigate(PaneId::Left, other.path().into()));
    ux_tests::wait(|| {
        app.model().pane(PaneId::Left).active().path == VPath::from(other.path())
            && !app.model().pane(PaneId::Left).loading
    });
    app.emit(AppMsg::SelectTab(PaneId::Left, 0));
    ux_tests::wait(|| {
        app.model().pane(PaneId::Left).active_tab == 0 && !app.model().pane(PaneId::Left).loading
    });
    if app.model().dual_pane {
        app.emit(AppMsg::ToggleDualPane);
        settle();
    }
    for mode in [
        PaneViewMode::List,
        PaneViewMode::Grid,
        PaneViewMode::Columns,
    ] {
        app.emit(AppMsg::SetViewMode(mode));
        ux_tests::wait(|| {
            app.model().pane(PaneId::Left).view_mode == mode
                && !app.model().pane(PaneId::Left).loading
        });
        settle();
        click(&app, 2, false, false);
        assert_selection(&app, &[2], false);
        let step = if mode == PaneViewMode::Grid {
            app.model().pane(PaneId::Left).grid_columns
        } else {
            1
        };
        command(&app, CommandId::CursorDown);
        assert_eq!(cursor(&app), 2 + step);
        assert_follows(&app);
        snapshot(&app, &format!("selection-follows-{mode:?}"));
        for action in [
            CommandId::CursorUp,
            CommandId::CursorLast,
            CommandId::CursorFirst,
            CommandId::CursorPageDown,
            CommandId::CursorPageUp,
        ] {
            command(&app, action);
            assert_follows(&app);
        }
        if mode == PaneViewMode::Grid {
            command(&app, CommandId::CursorRight);
            assert_follows(&app);
            command(&app, CommandId::CursorLeft);
            assert_follows(&app);
        }
        click(&app, 0, false, false);
        app.emit(AppMsg::AppendFilter('z'));
        settle();
        assert!(cursor(&app) >= 30);
        assert_follows(&app);

        // Insert/Shift+Space marks the current item even when it already has
        // the ordinary single-selection highlight; returning to it keeps the mark.
        click(&app, 2, false, false);
        command(&app, CommandId::ToggleSelection);
        assert_selection(&app, &[2], true);
        app.emit(AppMsg::MoveCursor(-1, false));
        settle();
        assert_eq!(cursor(&app), 2);
        command(&app, CommandId::CursorDown);
        assert_selection(&app, &[2], true);

        // Ctrl-click reduction to one item is still an intentional batch mark.
        click(&app, 0, false, false);
        click(&app, 2, true, false);
        assert_selection(&app, &[0, 2], true);
        click(&app, 0, true, false);
        assert_selection(&app, &[2], true);
        command(&app, CommandId::CursorLast);
        assert_selection(&app, &[2], true);
        // A plain click on the already-marked item returns to ordinary selection.
        click(&app, 2, false, false);
        assert_selection(&app, &[2], false);
        command(&app, CommandId::CursorDown);
        assert_follows(&app);

        click(&app, 1, false, false);
        click(&app, 4, false, true);
        assert_selection(&app, &[1, 2, 3, 4], true);
        command(&app, CommandId::CursorLast);
        assert_selection(&app, &[1, 2, 3, 4], true);
        click(&app, 2, false, false);
        app.emit(AppMsg::MoveCursor(1, true));
        settle();
        assert_selection(&app, &[2, 3], true);
        app.emit(AppMsg::MoveCursor(-1, true));
        settle();
        assert_selection(&app, &[2], true);
        command(&app, CommandId::CursorLast);
        assert_selection(&app, &[2], true);

        let generation = app.model().pane(PaneId::Left).generation;
        app.emit(AppMsg::Refresh(PaneId::Left));
        ux_tests::wait(|| {
            app.model().pane(PaneId::Left).generation > generation
                && !app.model().pane(PaneId::Left).loading
        });
        settle();
        assert_selection(&app, &[2], true);
        command(&app, CommandId::CursorFirst);
        assert_selection(&app, &[2], true);
        let tab = app.model().pane(PaneId::Left).active_tab;
        app.emit(AppMsg::SelectTab(PaneId::Left, 1));
        ux_tests::wait(|| {
            app.model().pane(PaneId::Left).active_tab != tab
                && !app.model().pane(PaneId::Left).loading
        });
        app.emit(AppMsg::SelectTab(PaneId::Left, tab));
        ux_tests::wait(|| {
            app.model().pane(PaneId::Left).active_tab == tab
                && !app.model().pane(PaneId::Left).loading
        });
        settle();
        assert_selection(&app, &[2], true);
        command(&app, CommandId::CursorLast);
        assert_selection(&app, &[2], true);
        snapshot(&app, &format!("marked-selection-{mode:?}"));
        command(&app, CommandId::ClearSelection);
        command(&app, CommandId::CursorFirst);
        assert_follows(&app);
    }
    // Switching layouts must keep the ordinary highlight and focus together.
    click(&app, 2, false, false);
    for mode in [
        PaneViewMode::Grid,
        PaneViewMode::List,
        PaneViewMode::Columns,
    ] {
        app.emit(AppMsg::SetViewMode(mode));
        ux_tests::wait(|| {
            app.model().pane(PaneId::Left).view_mode == mode
                && !app.model().pane(PaneId::Left).loading
        });
        settle();
        assert_eq!(cursor(&app), 2, "{mode:?}: view switch cursor");
        snapshot(&app, &format!("view-switch-{mode:?}"));
        assert_follows(&app);
    }
    command(&app, CommandId::ToggleSelection);
    for mode in [
        PaneViewMode::List,
        PaneViewMode::Grid,
        PaneViewMode::Columns,
    ] {
        app.emit(AppMsg::SetViewMode(mode));
        ux_tests::wait(|| {
            app.model().pane(PaneId::Left).view_mode == mode
                && !app.model().pane(PaneId::Left).loading
        });
        settle();
        command(&app, CommandId::CursorLast);
        assert_selection(&app, &[2], true);
    }
    app.widget().close();
}
