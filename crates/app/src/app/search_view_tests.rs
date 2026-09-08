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
    app.widgets()
        .search
        .results
        .emit_by_name::<()>("activate", &[&0_u32]);
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
    app.emit(AppMsg::OpenSearchResult(target));
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
        .results
        .emit_by_name::<()>("activate", &[&9_999_u32]);
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
    let Some(directory) = std::env::var_os("COMMANDER_SEARCH_SNAPSHOT_DIR") else {
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
