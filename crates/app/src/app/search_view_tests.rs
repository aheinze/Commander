use super::*;
use relm4::{Component, ComponentController};

#[track_caller]
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let context = glib::MainContext::default();
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "search reveal timed out at {}",
            std::panic::Location::caller()
        );
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn click_result(app: &relm4::Controller<AppModel>, path: &VPath, kind: EntryKind) {
    app.emit(AppMsg::OpenSearch(false));
    app.emit(AppMsg::SearchReady {
        generation: app.model().search_generation,
        result: Ok(SearchResults {
            hits: vec![SearchHit {
                path: path.clone(),
                kind,
                size: 1,
                content_match: false,
            }],
            ..SearchResults::default()
        }),
    });
    wait_until(|| {
        app.model().search_open
            && app
                .model()
                .search_results
                .hits
                .first()
                .is_some_and(|hit| hit.path == *path)
            && app.widgets().search.result_store.n_items() == 1
    });
    app.widgets().search.actions.selection.select_item(0, true);
    app.widgets().search.actions.reveal.emit_clicked();
    wait_until(|| {
        let model = app.model();
        !model.search_open
            && !model.pane(PaneId::Left).loading
            && model.focused_path(PaneId::Left).as_ref() == Some(path)
            && model.operation_sources(PaneId::Left) == vec![path.clone()]
    });
    assert_eq!(
        app.model().pane(PaneId::Left).current_directory(),
        &path.parent().unwrap()
    );
    assert_eq!(app.model().pane(PaneId::Left).selection.len(), 1);
}

#[test]
#[ignore = "requires an isolated GTK session; run alone with --ignored --test-threads=1"]
fn gtk_search_click_reveals_and_selects_in_every_view() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("Results");
    let child = folder.join("Result folder");
    std::fs::create_dir_all(child.join("Nested")).unwrap();
    for index in 0..300 {
        std::fs::write(folder.join(format!("file-{index:03}.txt")), b"fixture").unwrap();
    }
    let target = VPath::from(folder.join("zz matching item ü.txt"));
    std::fs::write(target.as_path(), b"match").unwrap();
    let hidden = VPath::from(folder.join(".hidden-match"));
    std::fs::write(hidden.as_path(), b"hidden match").unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let mut session = SessionState {
        dual_pane: false,
        sidebar_visible: false,
        preview_visible: false,
        window_width: 900,
        window_height: 600,
        ..SessionState::default()
    };
    session.left.view_mode = PaneViewMode::List;
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(folder.as_path())),
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
    wait_until(|| !app.model().pane(PaneId::Left).loading);

    exercise_large_results_and_cancellation(&app, &target);

    for mode in [
        PaneViewMode::List,
        PaneViewMode::Grid,
        PaneViewMode::Columns,
    ] {
        app.emit(AppMsg::SetViewMode(mode));
        wait_until(|| {
            app.model().pane(PaneId::Left).view_mode == mode
                && !app.model().pane(PaneId::Left).loading
        });
        if mode == PaneViewMode::Columns {
            let row = app.model().pane(PaneId::Left).miller_columns[0]
                .listing
                .as_ref()
                .unwrap()
                .rows()
                .position(|entry| entry.name() == OsStr::new("Result folder"))
                .unwrap() as u32;
            app.emit(AppMsg::MillerOpen(PaneId::Left, 0, row));
            wait_until(|| {
                app.model().pane(PaneId::Left).miller_columns.len() == 2
                    && !app.model().pane(PaneId::Left).loading
            });
        }
        click_result(&app, &target, EntryKind::File);
        wait_until(|| {
            let widgets = app.widgets();
            let pane = &widgets.panes[0];
            match mode {
                PaneViewMode::List => pane.column_view.vadjustment().unwrap().value() > 0.0,
                PaneViewMode::Grid => pane.grid_view.vadjustment().unwrap().value() > 0.0,
                PaneViewMode::Columns => pane
                    .miller_columns
                    .last()
                    .and_then(|column| column.scroll.as_ref())
                    .is_some_and(|scroll| scroll.vadjustment().value() > 0.0),
            }
        });
        let selected = {
            let widgets = app.widgets();
            if mode == PaneViewMode::Columns {
                assert_eq!(
                    app.model().pane(PaneId::Left).miller_columns.len(),
                    1,
                    "a saved child branch must not replace the containing folder"
                );
                widgets.panes[0].miller_columns[0].model.clone().unwrap()
            } else {
                widgets.panes[0].model.clone()
            }
        };
        assert!(selected.is_selected(app.model().pane(PaneId::Left).cursor_row));
    }
    click_result(&app, &VPath::from(child.as_path()), EntryKind::Directory);
    assert_eq!(
        app.model().pane(PaneId::Left).miller_columns.len(),
        1,
        "a folder hit is selected in its parent, without opening a child column"
    );
    assert!(!app.model().pane(PaneId::Left).show_hidden);
    click_result(&app, &hidden, EntryKind::File);
    assert!(app.model().pane(PaneId::Left).show_hidden);

    // An existing pane filter must not keep the requested item out of view.
    app.emit(AppMsg::SetPaneFilter(PaneId::Left, "file-000".into()));
    wait_until(|| {
        !app.model().pane(PaneId::Left).filtering
            && app.model().pane(PaneId::Left).filter_query == "file-000"
    });
    click_result(&app, &target, EntryKind::File);
    assert!(app.model().pane(PaneId::Left).filter_query.is_empty());

    // Navigating again before the listing completes cancels the pending reveal.
    app.emit(AppMsg::SearchAction(search_actions::Request {
        command: CommandId::Reveal,
        hits: vec![SearchHit {
            path: target,
            kind: EntryKind::File,
            size: 0,
            content_match: false,
        }],
        pane: PaneId::Left,
        destination: VPath::from(fixture.path()),
    }));
    app.emit(AppMsg::Navigate(PaneId::Left, VPath::from(fixture.path())));
    wait_until(|| {
        app.model().pane(PaneId::Left).current_directory() == &VPath::from(fixture.path())
            && !app.model().pane(PaneId::Left).loading
    });
    assert!(app.model().pane(PaneId::Left).pending_reveal.is_none());
    assert!(app.model().pane(PaneId::Left).selection.is_empty());
    app.widget().close();
}

fn count_result_rows(widget: &gtk::Widget) -> usize {
    let mut count = usize::from(widget.has_css_class("search-result-row"));
    let mut child = widget.first_child();
    while let Some(current) = child {
        count += count_result_rows(&current);
        child = current.next_sibling();
    }
    count
}

fn exercise_large_results_and_cancellation(app: &relm4::Controller<AppModel>, target: &VPath) {
    app.emit(AppMsg::OpenSearch(false));
    let mut hits: Vec<_> = (0..9_999)
        .map(|index| SearchHit {
            path: target.parent().unwrap().join_name(
                format!("document-{index:05}-{}-ü.txt", "long-name-".repeat(16)).as_ref(),
            ),
            kind: EntryKind::File,
            size: index,
            content_match: true,
        })
        .collect();
    hits.push(SearchHit {
        path: target.clone(),
        kind: EntryKind::File,
        size: 5,
        content_match: false,
    });
    app.emit(AppMsg::SearchReady {
        generation: app.model().search_generation,
        result: Ok(SearchResults {
            hits,
            skipped_entries: 2,
            first_error: Some("A private folder could not be read".into()),
            limit_reached: true,
        }),
    });
    wait_until(|| {
        let widgets = app.widgets();
        widgets.search.result_store.n_items() == 10_000
            && count_result_rows(widgets.search.results.upcast_ref()) > 0
            && widgets.search.results.width() > 0
    });
    {
        let widgets = app.widgets();
        assert!(
            count_result_rows(widgets.search.results.upcast_ref()) < 1_000,
            "results must create only visible rows, not 10,000 widgets"
        );
        assert!(
            widgets.search.results.width() < 900,
            "long filenames must not widen the dialog"
        );
        assert!(widgets.search.status.text().contains("10000 results"));
        assert_eq!(widgets.search.status.text(), "10000 results");
        assert!(
            notifications::test_messages()
                .iter()
                .any(|message| message.contains("narrow your search")
                    && message.contains("A private folder could not be read"))
        );
    }
    snapshot_search(app, "search-large-results");
    // Hits beyond the old 2,000-row cutoff remain actionable.
    app.widgets()
        .search
        .actions
        .selection
        .select_item(9_999, true);
    app.widgets().search.actions.reveal.emit_clicked();
    wait_until(|| {
        !app.model().search_open && app.model().focused_path(PaneId::Left).as_ref() == Some(target)
    });

    app.emit(AppMsg::OpenSearch(false));
    let generation = app.model().search_generation.wrapping_add(1);
    app.emit(AppMsg::RunSearch(SearchOptions {
        query: "file".into(),
        ..SearchOptions::default()
    }));
    app.emit(AppMsg::CancelSearch);
    app.emit(AppMsg::SearchReady {
        generation,
        result: Err("stale completion".into()),
    });
    wait_until(|| app.model().search_generation > generation && !app.model().search_loading);
    assert!(app.model().search_open);
    assert_ne!(
        app.model().search_error.as_deref(),
        Some("stale completion")
    );
    assert!(!app.widgets().search.stop.is_visible());

    let generation = app.model().search_generation;
    app.emit(AppMsg::CloseSearch);
    app.emit(AppMsg::OpenSearch(false));
    app.emit(AppMsg::SearchReady {
        generation,
        result: Err("closed completion".into()),
    });
    wait_until(|| {
        app.model().search_generation > generation
            && app.model().search_open
            && !app.widgets().search.closing_from_model.get()
            && app.widgets().search.dialog.is_visible()
    });
    assert_ne!(
        app.model().search_error.as_deref(),
        Some("closed completion")
    );
    app.emit(AppMsg::RunSearch(SearchOptions::default()));
    wait_until(|| {
        !app.model().search_loading
            && app.model().search_error.as_deref() == Some("Enter a name, path, or content query")
    });
    assert_eq!(app.widgets().search.result_store.n_items(), 0);
    app.widgets().search.dialog.close();
    wait_until(|| !app.model().search_open);
}

fn snapshot_search(app: &relm4::Controller<AppModel>, name: &str) {
    let Some(directory) = std::env::var_os("COMMANDER_SEARCH_SNAPSHOT_DIR")
        .or_else(|| std::env::var_os("COMMANDER_TEST_ARTIFACTS"))
    else {
        return;
    };
    // Allow the dialog's opening animation to settle before capturing it.
    let deadline = Instant::now() + Duration::from_millis(300);
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
    let child = gtk::prelude::GtkWindowExt::child(app.widget()).unwrap();
    let snapshot = gtk::Snapshot::new();
    app.widget().snapshot_child(&child, &snapshot);
    let node = snapshot.to_node().unwrap();
    app.widget()
        .renderer()
        .unwrap()
        .render_texture(&node, None)
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}

fn selected_results(app: &relm4::Controller<AppModel>, paths: &[&VPath]) {
    let widgets = app.widgets();
    widgets.search.actions.selection.unselect_all();
    for path in paths {
        let index = app
            .model()
            .search_results
            .hits
            .iter()
            .position(|hit| &hit.path == *path)
            .unwrap();
        widgets
            .search
            .actions
            .selection
            .select_item(index as u32, false);
    }
}

fn search_key(app: &relm4::Controller<AppModel>, key: gdk::Key, modifiers: gdk::ModifierType) {
    let list = app.widgets().search.results.clone();
    list.grab_focus();
    let controllers = list.observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(controller) = controllers
            .item(index)
            .and_downcast::<gtk::EventControllerKey>()
            && controller.propagation_phase() == gtk::PropagationPhase::Capture
        {
            assert!(controller.emit_by_name::<bool>("key-pressed", &[&key, &0_u32, &modifiers]));
            return;
        }
    }
    panic!("Search must own its keyboard controls");
}

fn search_button(widget: &impl IsA<gtk::Widget>, label: &str) -> gtk::Button {
    fn find(widget: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(button) = widget.downcast_ref::<gtk::Button>()
            && (button.label().as_deref() == Some(label)
                || button.tooltip_text().as_deref() == Some(label))
        {
            return Some(button.clone());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(button) = find(&widget, label) {
                return Some(button);
            }
        }
        None
    }
    find(widget.as_ref(), label).unwrap_or_else(|| panic!("Missing action: {label}"))
}

#[test]
#[ignore = "requires an isolated GTK session; run alone with --ignored --test-threads=1"]
fn gtk_search_actionable_results_keep_targets_and_refresh_after_changes() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("Search here");
    let destination = fixture.path().join("Destination");
    let folder = VPath::from(root.join("match folder"));
    for path in [
        root.join("First"),
        root.join("Second"),
        folder.as_path().to_owned(),
        destination.clone(),
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
    let alpha = VPath::from(root.join("First/match alpha.txt"));
    let beta = VPath::from(root.join("Second/match β.txt"));
    let child = VPath::from(folder.as_path().join("match child.txt"));
    let keep = VPath::from(root.join("untouched.txt"));
    for path in [&alpha, &beta, &child, &keep] {
        std::fs::write(path.as_path(), path.to_string()).unwrap();
    }
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(root.as_path())),
                right: Some(VPath::from(destination.as_path())),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                sidebar_visible: false,
                preview_visible: false,
                window_width: 1000,
                window_height: 780,
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
        !app.model().pane(PaneId::Left).loading && !app.model().pane(PaneId::Right).loading
    });
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((keep.clone(), EntryKind::File)),
    ));
    app.emit(AppMsg::OpenSearch(false));
    wait_until(|| app.model().search_open);
    app.widgets().search.query.set_text("match");
    app.widgets()
        .search
        .query
        .emit_by_name::<()>("activate", &[]);
    wait_until(|| !app.model().search_loading && app.widgets().search.result_store.n_items() == 4);
    assert!(!app.widgets().search.results.is_single_click_activate());
    assert!(!app.widgets().search.actions.copy.is_sensitive());
    selected_results(&app, &[&alpha, &beta]);
    assert_eq!(app.widgets().search.actions.count.text(), "2 selected");
    assert!(!app.widgets().search.actions.reveal.is_sensitive());

    // A refreshed/reordered snapshot preserves file identity, not row indexes.
    let mut reversed = app.model().search_results.clone();
    reversed.hits.reverse();
    app.emit(AppMsg::SearchReady {
        generation: app.model().search_generation,
        result: Ok(reversed.clone()),
    });
    wait_until(|| *app.widgets().search.rendered_results.borrow() == reversed.hits);
    assert_eq!(
        app.widgets()
            .search
            .actions
            .hits()
            .iter()
            .map(|h| h.path.clone())
            .collect::<BTreeSet<_>>(),
        [alpha.clone(), beta.clone()].into_iter().collect()
    );
    snapshot_search(&app, "search-actions-selected-dark");

    // The desktop clipboard contains both paths, even across different parents.
    search_key(&app, gdk::Key::c, gdk::ModifierType::CONTROL_MASK);
    wait_until(|| app.model().clipboard_provider.is_some());
    assert!(
        gdk::Display::default()
            .unwrap()
            .clipboard()
            .formats()
            .contain_mime_type("text/uri-list")
    );
    selected_results(&app, &[&folder]);
    search_key(&app, gdk::Key::v, gdk::ModifierType::CONTROL_MASK);
    wait_until(|| {
        folder.as_path().join("match alpha.txt").exists()
            && folder.as_path().join("match β.txt").exists()
            && app.model().active_operations == 0
            && !app.model().search_loading
    });
    assert!(keep.as_path().exists());
    assert_eq!(
        app.model().operation_sources(PaneId::Left),
        vec![keep.clone()]
    );
    assert_eq!(app.widgets().search.actions.hits()[0].path, folder);
    search_key(&app, gdk::Key::z, gdk::ModifierType::CONTROL_MASK);
    wait_until(|| {
        !folder.as_path().join("match alpha.txt").exists()
            && !app.model().history_busy
            && !app.model().search_loading
    });

    // Open the real menu, then change selection. The existing menu keeps its targets.
    selected_results(&app, &[&alpha, &beta]);
    app.widgets().search.actions.more.emit_clicked();
    let menu = app
        .widgets()
        .search
        .actions
        .more
        .last_child()
        .and_downcast::<gtk::Popover>()
        .unwrap();
    wait_until(|| menu.is_mapped());
    assert!(menu.has_css_class("file-context-menu"));
    let copy = search_button(
        &menu,
        &format!("Copy to other pane: {}", destination.display()),
    );
    snapshot_search_menu(&app, &menu, "search-actions-menu-dark");
    selected_results(&app, &[&folder]);
    copy.emit_clicked();
    wait_until(|| {
        destination.join("match alpha.txt").exists()
            && destination.join("match β.txt").exists()
            && app.model().active_operations == 0
            && !app.model().search_loading
    });
    assert!(!destination.join("match folder").exists());
    assert!(alpha.as_path().exists() && beta.as_path().exists());
    search_key(&app, gdk::Key::z, gdk::ModifierType::CONTROL_MASK);
    wait_until(|| {
        !destination.join("match alpha.txt").exists()
            && !app.model().history_busy
            && !app.model().search_loading
    });

    // A multi-directory move has a usable undo record and refreshes the search.
    selected_results(&app, &[&alpha, &beta]);
    app.widgets().search.actions.send(CommandId::Move);
    wait_until(|| {
        !alpha.as_path().exists()
            && !beta.as_path().exists()
            && app.model().active_operations == 0
            && !app.model().search_loading
    });
    assert!(
        app.model()
            .search_results
            .hits
            .iter()
            .all(|h| h.path != alpha && h.path != beta)
    );
    search_key(&app, gdk::Key::z, gdk::ModifierType::CONTROL_MASK);
    wait_until(|| {
        alpha.as_path().exists()
            && beta.as_path().exists()
            && !app.model().history_busy
            && !app.model().search_loading
    });

    // Rename stays in the search and refreshes the original root.
    selected_results(&app, &[&alpha]);
    app.widgets().search.actions.send(CommandId::Rename);
    wait_until(|| {
        app.widget()
            .visible_dialog()
            .is_some_and(|d| d.has_css_class("alert-sheet"))
    });
    let rename = app.widget().visible_dialog().unwrap();
    fn entry(widget: &gtk::Widget) -> Option<gtk::Entry> {
        if let Ok(entry) = widget.clone().downcast::<gtk::Entry>() {
            return Some(entry);
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(entry) = entry(&widget) {
                return Some(entry);
            }
        }
        None
    }
    entry(rename.upcast_ref())
        .unwrap()
        .set_text("match renamed.txt");
    search_button(&rename, "Rename").emit_clicked();
    let renamed = VPath::from(root.join("First/match renamed.txt"));
    wait_until(|| {
        renamed.as_path().exists()
            && !app.model().search_loading
            && app
                .model()
                .search_results
                .hits
                .iter()
                .any(|h| h.path == renamed)
    });
    assert!(app.model().search_open);

    // Trash excludes a selected folder's child and can be undone as one job.
    selected_results(&app, &[&folder, &child]);
    app.widgets().search.actions.send(CommandId::Trash);
    wait_until(|| {
        !folder.as_path().exists()
            && app.model().active_operations == 0
            && !app.model().search_loading
    });
    assert!(app.model().operations.values().all(|op| op.error.is_none()));
    search_key(&app, gdk::Key::z, gdk::ModifierType::CONTROL_MASK);
    wait_until(|| {
        child.as_path().exists() && !app.model().history_busy && !app.model().search_loading
    });

    // Destructive confirmation keeps the reviewed path, even if selection changes.
    selected_results(&app, &[&beta]);
    app.widgets()
        .search
        .actions
        .send(CommandId::DeletePermanent);
    wait_until(|| {
        app.widget()
            .visible_dialog()
            .is_some_and(|d| d.has_css_class("alert-sheet"))
    });
    let confirm = app.widget().visible_dialog().unwrap();
    assert!(beta.as_path().exists());
    selected_results(&app, &[&renamed]);
    search_button(&confirm, "Cancel").emit_clicked();
    wait_until(|| {
        app.widget()
            .visible_dialog()
            .is_some_and(|d| d.title() == "Search")
    });
    assert!(beta.as_path().exists());
    selected_results(&app, &[&beta]);
    app.widgets()
        .search
        .actions
        .send(CommandId::DeletePermanent);
    wait_until(|| {
        app.widget()
            .visible_dialog()
            .is_some_and(|d| d.has_css_class("alert-sheet"))
    });
    let confirm = app.widget().visible_dialog().unwrap();
    selected_results(&app, &[&renamed]);
    search_button(&confirm, "Delete").emit_clicked();
    wait_until(|| {
        !beta.as_path().exists()
            && app.model().active_operations == 0
            && !app.model().search_loading
    });
    assert!(renamed.as_path().exists() && keep.as_path().exists());

    // Compact/light controls, keyboard menu, and default activation of a folder.
    apply_appearance(AppearanceMode::Light);
    app.widgets().search.dialog.set_content_width(620);
    app.widget().set_default_size(760, 740);
    selected_results(&app, &[&folder]);
    snapshot_search(&app, "search-actions-compact-light");
    search_key(&app, gdk::Key::F10, gdk::ModifierType::SHIFT_MASK);
    let menu = app
        .widgets()
        .search
        .results
        .last_child()
        .and_downcast::<gtk::Popover>()
        .unwrap();
    wait_until(|| menu.is_mapped());
    app.widget().set_focus_visible(true);
    assert!(search_button(&menu, "Open").has_focus());
    let controllers = menu.observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(controller) = controllers
            .item(index)
            .and_downcast::<gtk::EventControllerKey>()
            && controller.propagation_phase() == gtk::PropagationPhase::Capture
        {
            assert!(controller.emit_by_name::<bool>(
                "key-pressed",
                &[&gdk::Key::Down, &0_u32, &gdk::ModifierType::empty()]
            ));
        }
    }
    assert!(search_button(&menu, "Show in folder").has_focus());
    snapshot_search_menu(&app, &menu, "search-actions-keyboard-menu-light");
    app.widgets().search.actions.close_menu();
    let position = app
        .model()
        .search_results
        .hits
        .iter()
        .position(|h| h.path == folder)
        .unwrap() as u32;
    app.widgets()
        .search
        .results
        .emit_by_name::<()>("activate", &[&position]);
    wait_until(|| {
        !app.model().search_open && app.model().pane(PaneId::Left).current_directory() == &folder
    });
    exercise_archive_search(&app, &fixture);
    app.widget().close();
}

fn exercise_archive_search(app: &relm4::Controller<AppModel>, fixture: &tempfile::TempDir) {
    let docs = fixture.path().join("Archived documents");
    std::fs::create_dir(&docs).unwrap();
    for name in ["match-one.txt", "match-two.txt"] {
        std::fs::write(docs.join(name), b"archived result").unwrap();
    }
    let archive = VPath::from(fixture.path().join("Results.zip"));
    let cancel = CancelToken::new();
    crate::archive::create_archive(
        &LocalFs,
        &[VPath::from(docs.as_path())],
        &archive,
        crate::archive::ArchiveFormat::Zip,
        &mut crate::archive::ArchiveTask::new(&cancel),
    )
    .unwrap();
    app.emit(AppMsg::Navigate(PaneId::Left, archive));
    wait_until(|| {
        app.model()
            .is_archive_browse_path(app.model().pane(PaneId::Left).current_directory())
            && !app.model().pane(PaneId::Left).loading
    });
    app.emit(AppMsg::OpenSearch(false));
    app.emit(AppMsg::RunSearch(SearchOptions {
        query: "match".into(),
        ..SearchOptions::default()
    }));
    wait_until(|| !app.model().search_loading && app.model().search_results.hits.len() == 2);
    let old_root = app.model().search_session.as_ref().unwrap().root.clone();
    let first = app.model().search_results.hits[0].path.clone();
    selected_results(app, &[&first]);
    app.widgets().search.actions.send(CommandId::Cut);
    wait_until(|| {
        notifications::test_messages()
            .iter()
            .any(|s| s.contains("not available inside archives"))
    });
    assert!(first.as_path().exists());
    app.widgets().search.actions.send(CommandId::Trash);
    wait_until(|| {
        app.widget()
            .visible_dialog()
            .is_some_and(|d| d.has_css_class("alert-sheet"))
    });
    let confirm = app.widget().visible_dialog().unwrap();
    search_button(&confirm, "Remove").emit_clicked();
    wait_until(|| {
        app.model().active_operations == 0
            && !app.model().search_loading
            && app.model().search_session.as_ref().unwrap().root != old_root
            && app.model().search_results.hits.len() == 1
    });
    assert!(
        app.model()
            .is_archive_browse_path(&app.model().search_results.hits[0].path)
    );
    search_key(app, gdk::Key::z, gdk::ModifierType::CONTROL_MASK);
    wait_until(|| {
        !app.model().history_busy
            && !app.model().search_loading
            && app.model().search_results.hits.len() == 2
    });
    assert!(docs.join("match-one.txt").exists() && docs.join("match-two.txt").exists());
}

fn snapshot_search_menu(app: &relm4::Controller<AppModel>, menu: &gtk::Popover, name: &str) {
    wait_until(|| menu.is_mapped() && menu.width() > 0);
    // Native popovers own a separate surface, so a window snapshot omits them.
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(menu.width() <= 410);
    assert!(menu.height() <= app.widget().height());
    let child = menu.first_child().unwrap();
    let snapshot = gtk::Snapshot::new();
    menu.snapshot_child(&child, &snapshot);
    let Some(directory) = std::env::var_os("COMMANDER_SEARCH_SNAPSHOT_DIR") else {
        return;
    };
    menu.renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}
