use super::*;
use relm4::{Component, ComponentController};

fn listing(path: &std::path::Path) -> Arc<Listing> {
    ListingTask::spawn(
        Arc::new(LocalFs),
        ListingRequest {
            path: VPath::from(path),
            sort: SortSpec::default(),
        },
    )
    .unwrap()
    .wait_complete()
    .unwrap()
    .0
}

fn select(listing: &Listing, names: &[&str]) -> Selection {
    let mut selection = Selection::new();
    selection.replace(
        listing
            .rows()
            .filter(|entry| names.iter().any(|name| entry.name() == OsStr::new(name)))
            .map(|entry| SelectionKey::for_entry(listing.parent(), entry)),
    );
    selection
}

#[test]
fn totals_use_actual_sizes_without_cached_row_metadata() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), b"1234").unwrap();
    std::fs::write(dir.path().join("b"), b"123456").unwrap();
    std::fs::write(dir.path().join("empty"), b"").unwrap();
    let listing = listing(dir.path());
    assert!(
        listing
            .metadata(listing.source_index_at_row(0).unwrap())
            .is_none()
    );
    for (names, expected) in [(&["a"][..], 4), (&["a", "b"][..], 10), (&["empty"][..], 0)] {
        let result = measure_selected_entries(
            &LocalFs,
            vec![listing.clone()],
            &select(&listing, names),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(result.bytes, expected);
        assert_eq!(result.skipped, 0);
    }
    // A disappeared/unreadable selected item must not silently look like a full total.
    std::fs::remove_file(dir.path().join("b")).unwrap();
    let result = measure_selected_entries(
        &LocalFs,
        vec![listing.clone()],
        &select(&listing, &["a", "b"]),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(result.bytes, 4);
    assert_eq!(result.skipped, 1);
}

#[test]
fn folder_totals_include_hidden_content_and_do_not_follow_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir_all(folder.join("nested")).unwrap();
    std::fs::write(folder.join("a"), b"1234").unwrap();
    std::fs::write(folder.join("nested/.hidden"), b"123456").unwrap();
    std::fs::hard_link(folder.join("a"), folder.join("another-name")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("..", folder.join("nested/loop")).unwrap();
    let root = listing(dir.path());
    let result = measure_selected_entries(
        &LocalFs,
        vec![root.clone(), listing(&folder)],
        &select(&root, &["folder"]),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(result.bytes, 14 + if cfg!(unix) { 2 } else { 0 });
    assert_eq!(result.skipped, 0);
}

#[test]
fn cancelled_and_stale_results_cannot_replace_the_current_selection_size() {
    let mut state = PaneState::from_session(&PaneSession::default(), VPath::from("/"));
    state.selection_size.loading = true;
    let generation = state.selection_size.generation;
    let cancel = CancelToken::new();
    state.selection_size.cancel = Some(cancel.clone());
    state.cancel_work();
    assert!(cancel.is_cancelled());
    assert!(measure_selected_entries(&LocalFs, vec![], &Selection::new(), &cancel).is_err());
    // A result queued just before cancellation arrives after the next selection starts.
    state.selection_size.loading = true;
    state.finish_selection_size(
        generation,
        Ok(FolderMeasureResult {
            bytes: 999,
            ..Default::default()
        }),
    );
    assert!(state.selection_size.result.is_none());
    state.finish_selection_size(
        state.selection_size.generation,
        Ok(FolderMeasureResult {
            bytes: 4,
            skipped: 1,
            ..Default::default()
        }),
    );
    assert_eq!(state.selection_size_label(), "4 B (partial)");
}

#[track_caller]
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let context = glib::MainContext::default();
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "selection size timed out at {}",
            std::panic::Location::caller()
        );
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[track_caller]
fn wait_status(app: &relm4::Controller<AppModel>, suffix: &str) {
    wait_until(|| app.widgets().panes[0].status.label().ends_with(suffix));
}

#[test]
#[ignore = "requires an isolated GTK session; run alone with --ignored --test-threads=1"]
fn gtk_selection_totals_update_in_every_view() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("folder/nested")).unwrap();
    std::fs::write(dir.path().join("folder/nested/content"), b"1234567890").unwrap();
    std::fs::write(dir.path().join("a"), b"1234").unwrap();
    std::fs::write(dir.path().join("b"), b"123456").unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let mut session = SessionState {
        sidebar_visible: false,
        preview_visible: false,
        dual_pane: false,
        ..SessionState::default()
    };
    session.left.view_mode = PaneViewMode::List;
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(dir.path())),
                right: Some(VPath::from(dir.path())),
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
        let model = {
            let widgets = app.widgets();
            if mode == PaneViewMode::Columns {
                widgets.panes[0].miller_columns[0].model.clone().unwrap()
            } else {
                widgets.panes[0].model.clone()
            }
        };
        assert!(model.select_item(1, true)); // a (folders first)
        wait_status(&app, "1 selected · 4 B");
        assert!(model.select_item(2, false)); // add b
        wait_status(&app, "2 selected · 10 B");
        assert!(model.unselect_item(1));
        wait_status(&app, "1 selected · 6 B");
        assert!(model.unselect_all());
        wait_status(&app, "0 selected");
        app.emit(AppMsg::SelectAllActive);
        wait_status(&app, "3 selected · 20 B");
        app.emit(AppMsg::ClearSelectionActive);
        wait_status(&app, "0 selected");
    }
    app.emit(AppMsg::MillerOpen(PaneId::Left, 0, 0));
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns.len() == 2
            && !app.model().pane(PaneId::Left).loading
    });
    wait_status(&app, "1 selected · 10 B");

    // Selected files change size without a new click: native folder updates refresh the total.
    app.emit(AppMsg::OpenSearchResult(VPath::from(dir.path().join("a"))));
    wait_until(|| {
        !app.model().pane(PaneId::Left).loading
            && app.model().pane(PaneId::Left).current_directory() == &VPath::from(dir.path())
    });
    app.emit(AppMsg::SetViewMode(PaneViewMode::List));
    wait_until(|| {
        app.model().pane(PaneId::Left).view_mode == PaneViewMode::List
            && !app.model().pane(PaneId::Left).loading
    });
    app.widgets().panes[0].model.select_item(1, true);
    wait_status(&app, "1 selected · 4 B");
    std::fs::write(dir.path().join("a"), b"12345678").unwrap();
    wait_status(&app, "1 selected · 8 B");
    app.emit(AppMsg::Navigate(
        PaneId::Left,
        VPath::from(dir.path().join("folder/nested")),
    ));
    wait_status(&app, "0 selected");
    let right_listing = app
        .model()
        .pane(PaneId::Right)
        .active()
        .listing
        .clone()
        .unwrap();
    app.emit(AppMsg::SelectionChanged(
        PaneId::Right,
        select(&right_listing, &["b"]),
        Some(2),
    ));
    wait_until(|| {
        app.widgets().panes[1]
            .status
            .label()
            .ends_with("1 selected · 6 B")
    });
    assert!(
        app.widgets().panes[0]
            .status
            .label()
            .ends_with("0 selected")
    );
    app.widget().close();
}
