use super::*;

fn columns_test_app(root: &std::path::Path) -> relm4::Controller<AppModel> {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(root.into()),
                right: Some(root.into()),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                left: PaneSession {
                    view_mode: PaneViewMode::Columns,
                    ..PaneSession::default()
                },
                dual_pane: false,
                sidebar_visible: false,
                preview_visible: false,
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
        !app.model().pane(PaneId::Left).loading
            && app
                .model()
                .pane(PaneId::Left)
                .miller_columns
                .first()
                .is_some_and(|column| column.listing.is_some())
    });
    drain_frames();
    app
}

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_miller_keyboard_uses_clicked_ancestor() {
    let fixture = tempfile::tempdir().unwrap();
    let alpha = fixture.path().join("Alpha");
    let beta = fixture.path().join("Beta");
    std::fs::create_dir(&alpha).unwrap();
    std::fs::create_dir(&beta).unwrap();
    std::fs::write(alpha.join("a.txt"), b"a").unwrap();
    std::fs::write(alpha.join("b.txt"), b"b").unwrap();
    let app = columns_test_app(fixture.path());
    click_miller_folder(&app, 0, "Alpha");
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns.len() == 2
            && !app.model().pane(PaneId::Left).filtering
    });
    click_miller_folder(&app, 0, "Alpha");
    drain_frames();
    assert_miller_focus(&app, 0, "Alpha");
    // This is the exact command dispatched by the window's Down shortcut.
    app.emit(AppMsg::ExecuteCommand(CommandId::CursorDown));
    drain_frames();
    assert_eq!(
        app.model().focused_path(PaneId::Left),
        Some(beta.into()),
        "Down must move within the column that has keyboard focus"
    );
}

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_miller_typeahead_finds_visible_child() {
    let fixture = tempfile::tempdir().unwrap();
    let alpha = fixture.path().join("Alpha");
    std::fs::create_dir(&alpha).unwrap();
    std::fs::write(alpha.join("a.txt"), b"a").unwrap();
    std::fs::write(alpha.join("zebra.txt"), b"z").unwrap();
    std::fs::write(alpha.join("zoo.txt"), b"zz").unwrap();
    std::fs::write(alpha.join("Überblick.txt"), b"unicode").unwrap();
    let app = columns_test_app(fixture.path());
    click_miller_folder(&app, 0, "Alpha");
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns.len() == 2
            && !app.model().pane(PaneId::Left).filtering
    });
    select_miller_item(&app, 1, "a.txt", false, false);
    // The unmodified character-key handler dispatches AppendFilter for type-ahead.
    app.emit(AppMsg::AppendFilter('z'));
    drain_frames();
    assert_miller_focus(&app, 1, "zebra.txt");
    app.emit(AppMsg::AppendFilter('z'));
    drain_frames();
    assert_miller_focus(&app, 1, "zoo.txt");
    app.emit(AppMsg::AppendFilter('z'));
    drain_frames();
    assert_miller_focus(&app, 1, "zebra.txt");
    // A new column resets the prefix, including for non-ASCII characters.
    click_miller_folder(&app, 0, "Alpha");
    drain_frames();
    select_miller_item(&app, 1, "a.txt", false, false);
    app.emit(AppMsg::AppendFilter('ü'));
    drain_frames();
    assert_miller_focus(&app, 1, "Überblick.txt");
}

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_miller_enter_uses_clicked_ancestor() {
    let fixture = tempfile::tempdir().unwrap();
    let alpha = fixture.path().join("Alpha");
    std::fs::create_dir_all(alpha.join("Child")).unwrap();
    let app = columns_test_app(fixture.path());
    click_miller_folder(&app, 0, "Alpha");
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns.len() == 2
            && !app.model().pane(PaneId::Left).filtering
    });
    click_miller_folder(&app, 0, "Alpha");
    drain_frames();
    assert_miller_focus(&app, 0, "Alpha");
    app.emit(AppMsg::ExecuteCommand(CommandId::Open));
    drain_frames();
    assert_eq!(
        app.model().pane(PaneId::Left).current_directory(),
        &VPath::from(alpha),
        "Enter should activate Alpha, which is already open, rather than Child in another column"
    );
}

#[test]
fn miller_navigation_snapshot_retains_ancestor_selection() {
    let fixture = tempfile::tempdir().unwrap();
    let child = fixture.path().join("Child");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(child.join("note.txt"), b"note").unwrap();
    let mut state = PaneState::from_session(&PaneSession::default(), VPath::from(fixture.path()));
    state.view_mode = PaneViewMode::Columns;
    state.miller_columns = vec![column(fixture.path()), column(&child)];
    let root = state.miller_columns[0].listing.as_ref().unwrap();
    let mut selected = Selection::new();
    selected.select(SelectionKey::for_entry(root.parent(), root.row(0).unwrap()));
    assert!(state.select_miller_rows(0, &VPath::from(fixture.path()), selected, Some(0)));
    assert_eq!(
        state.selected_miller_sources(),
        vec![VPath::from(child.as_path())]
    );
    let session = state.to_session();
    let mut restored = PaneState::from_session(&session, VPath::from(fixture.path()));
    assert_eq!(restored.miller_columns.len(), 2);
    for column in &mut restored.miller_columns {
        let entries = listing(column.path.as_path());
        column.base_listing = Some(entries.clone());
        column.listing = Some(entries);
        column.loading = false;
    }
    restored.restore_selection();
    assert_eq!(
        restored.selected_miller_sources(),
        vec![VPath::from(child.as_path())],
        "The selected ancestor must survive a saved-session round trip"
    );
}

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_miller_focus_selection_and_filters_survive_updates_and_tabs() {
    let fixture = tempfile::tempdir().unwrap();
    let alpha = fixture.path().join("Alpha");
    let beta = fixture.path().join("Beta");
    std::fs::create_dir(&alpha).unwrap();
    std::fs::create_dir(&beta).unwrap();
    std::fs::write(alpha.join("note.txt"), b"note").unwrap();
    std::fs::write(alpha.join("zebra.txt"), b"zebra").unwrap();
    let app = columns_test_app(fixture.path());
    click_miller_folder(&app, 0, "Alpha");
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns.len() == 2
            && !app.model().pane(PaneId::Left).filtering
    });
    click_miller_folder(&app, 0, "Alpha");
    drain_frames();
    let generation = app.model().pane(PaneId::Left).miller_generation;
    app.emit(AppMsg::Refresh(PaneId::Left));
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_generation > generation
            && !app.model().pane(PaneId::Left).loading
    });
    assert_miller_focus(&app, 0, "Alpha");
    assert_eq!(
        app.model().pane(PaneId::Left).active_miller_column(),
        Some(0)
    );
    std::fs::write(fixture.path().join("new.txt"), b"watch update").unwrap();
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns[0]
            .listing
            .as_ref()
            .unwrap()
            .len()
            == 3
    });
    drain_frames();
    assert_miller_focus(&app, 0, "Alpha");
    app.emit(AppMsg::SelectAllActive);
    drain_frames();
    assert_eq!(app.model().operation_sources(PaneId::Left).len(), 3);
    assert_eq!(
        app.widgets().panes[0].miller_columns[0]
            .model
            .as_ref()
            .unwrap()
            .stable_selection()
            .len(),
        3
    );
    assert!(
        app.widgets().panes[0].miller_columns[1]
            .model
            .as_ref()
            .unwrap()
            .stable_selection()
            .is_empty()
    );
    app.emit(AppMsg::ClearLayered);
    drain_frames();
    assert!(app.model().pane(PaneId::Left).selection.is_empty());
    click_miller_folder(&app, 0, "Alpha");
    drain_frames();
    app.emit(AppMsg::NewTab(PaneId::Left));
    wait_until(|| {
        app.model().pane(PaneId::Left).active_tab == 1
            && !app.model().pane(PaneId::Left).loading
            && !app.model().pane(PaneId::Left).filtering
    });
    app.emit(AppMsg::SelectTab(PaneId::Left, 0));
    wait_until(|| {
        app.model().pane(PaneId::Left).active_tab == 0
            && app.model().pane(PaneId::Left).miller_columns.len() == 2
            && !app.model().pane(PaneId::Left).loading
    });
    drain_frames();
    assert_eq!(
        app.model().operation_sources(PaneId::Left),
        vec![VPath::from(alpha.as_path())]
    );
    assert_eq!(
        app.model().pane(PaneId::Left).active_miller_column(),
        Some(0)
    );
    assert_miller_focus(&app, 0, "Alpha");
    // Right explicitly enters the already open child, and Enter acts on its row.
    app.emit(AppMsg::MoveCursorHorizontal(1, false));
    drain_frames();
    assert_eq!(
        app.model().pane(PaneId::Left).active_miller_column(),
        Some(1)
    );
    assert_miller_focus(&app, 1, "note.txt");
    app.widgets().global_search.grab_focus();
    app.widgets().global_search.set_text("missing");
    wait_until(|| {
        app.model().pane(PaneId::Left).filter_query == "missing"
            && !app.model().pane(PaneId::Left).filtering
    });
    assert!(
        app.model().pane(PaneId::Left).miller_columns[1]
            .listing
            .as_ref()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.model().pane(PaneId::Left).active_miller_column(),
        Some(1)
    );
    app.widgets().global_search.set_text("");
    wait_until(|| {
        app.model().pane(PaneId::Left).filter_query.is_empty()
            && !app.model().pane(PaneId::Left).filtering
    });
    select_miller_item(&app, 1, "note.txt", false, false);
    app.emit(AppMsg::AppendFilter('z'));
    drain_frames();
    assert_miller_focus(&app, 1, "zebra.txt");
    // Truncation removes the leaf's filter and resets the range anchor.
    app.widgets().global_search.grab_focus();
    app.widgets().global_search.set_text("zebra");
    wait_until(|| {
        app.model().pane(PaneId::Left).filter_query == "zebra"
            && !app.model().pane(PaneId::Left).filtering
    });
    click_miller_folder(&app, 0, "Alpha");
    drain_frames();
    app.emit(AppMsg::MoveCursorVertical(1, true));
    drain_frames();
    assert_eq!(app.model().pane(PaneId::Left).miller_columns.len(), 1);
    assert!(app.model().pane(PaneId::Left).filter_query.is_empty());
    assert_eq!(
        app.model().operation_sources(PaneId::Left),
        vec![VPath::from(alpha), VPath::from(beta)]
    );
    app.widget().close();
}

#[test]
fn miller_saved_paths_are_lossless_and_never_select_same_named_siblings() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = tempfile::tempdir().unwrap();
    let child = fixture
        .path()
        .join(OsString::from_vec(b"Child\xff".to_vec()));
    std::fs::create_dir(&child).unwrap();
    std::fs::write(fixture.path().join("note"), b"root").unwrap();
    std::fs::write(child.join("note"), b"child").unwrap();
    let mut state = PaneState::from_session(&PaneSession::default(), fixture.path().into());
    state.view_mode = PaneViewMode::Columns;
    state.miller_columns = vec![column(fixture.path()), column(&child)];
    let entries = state.miller_columns[1].listing.as_ref().unwrap();
    state.selection.select(SelectionKey::for_entry(
        entries.parent(),
        entries.row(0).unwrap(),
    ));
    state.focus_miller_column(1);
    let encoded = toml_edit::ser::to_string(&state.to_session()).unwrap();
    let session: PaneSession = toml_edit::de::from_str(&encoded).unwrap();
    for remove_child in [false, true] {
        let mut restored = PaneState::from_session(&session, "/unused".into());
        if remove_child {
            restored.miller_columns.truncate(1);
        }
        for column in &mut restored.miller_columns {
            column.listing = Some(listing(column.path.as_path()));
            column.loading = false;
        }
        restored.restore_selection();
        let expected = if remove_child {
            vec![]
        } else {
            vec![VPath::from(child.join("note"))]
        };
        assert_eq!(restored.selected_miller_sources(), expected);
    }
    // Sessions written before selected_paths existed still restore leaf names.
    let mut legacy = session;
    for location in legacy.locations.values_mut() {
        location.selected_paths.clear();
        location.focused_column = None;
    }
    let mut restored = PaneState::from_session(&legacy, "/unused".into());
    for column in &mut restored.miller_columns {
        column.listing = Some(listing(column.path.as_path()));
        column.loading = false;
    }
    restored.restore_selection();
    assert_eq!(
        restored.selected_miller_sources(),
        vec![VPath::from(child.join("note"))]
    );
}

#[test]
fn miller_branch_truncation_cancels_loading_without_leaving_busy_state() {
    let fixture = tempfile::tempdir().unwrap();
    let child = fixture.path().join("Child");
    std::fs::create_dir(&child).unwrap();
    let mut state = PaneState::from_session(&PaneSession::default(), fixture.path().into());
    state.view_mode = PaneViewMode::Columns;
    state.miller_columns = vec![column(fixture.path()), column(&child)];
    state.focus_miller_column(1);
    state.loading = true;
    state.filtering = true;
    state.filter_query = "old leaf filter".into();
    for column in &mut state.miller_columns {
        column.loading = true;
    }
    let cancel = CancelToken::new();
    state.miller_cancel = Some(cancel.clone());
    state.truncate_miller_branch(1);
    assert!(cancel.is_cancelled());
    assert!(!state.loading && !state.filtering);
    assert!(state.filter_query.is_empty());
    assert!(state.miller_columns.iter().all(|column| !column.loading));
    assert_eq!(state.active_miller_column(), Some(0));
    assert_eq!(state.miller_generation, 1);
}
