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

fn column(path: &std::path::Path) -> MillerColumnState {
    MillerColumnState {
        base_listing: Some(listing(path)),
        restore_name: None,
        scroll_y: 0,
        path: VPath::from(path),
        listing: Some(listing(path)),
        selected_row: Some(0),
        loading: false,
        error: None,
        width: 280,
    }
}

#[test]
fn pointer_selection_keeps_all_selected_files_and_cancels_discarded_columns() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    std::fs::write(dir.path().join("b.txt"), "b").unwrap();
    let mut state = PaneState::from_session(&PaneSession::default(), VPath::from(dir.path()));
    state.miller_columns = vec![column(dir.path()), column(dir.path())];
    let cancel = CancelToken::new();
    state.miller_cancel = Some(cancel.clone());
    let listing = state.miller_columns[0].listing.as_ref().unwrap();
    let mut selected = Selection::new();
    selected.replace(
        listing
            .rows()
            .map(|entry| SelectionKey::for_entry(listing.parent(), entry)),
    );
    assert!(state.select_miller_rows(0, &VPath::from(dir.path()), selected.clone(), Some(1)));
    assert_eq!(state.selection, selected);
    assert_eq!(state.miller_columns.len(), 1);
    assert_eq!(
        state.miller_focus.as_ref().unwrap().0,
        VPath::from(dir.path().join("b.txt"))
    );
    assert!(cancel.is_cancelled());
    assert_eq!(state.miller_generation, 1);
}

#[test]
fn stale_column_selection_cannot_change_the_current_folder() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    let mut state = PaneState::from_session(&PaneSession::default(), VPath::from(dir.path()));
    state.miller_columns = vec![column(dir.path())];
    assert!(!state.select_miller_rows(0, &VPath::from("/stale"), Selection::new(), Some(0)));
    assert!(!state.select_miller_rows(2, &VPath::from(dir.path()), Selection::new(), Some(0)));
    assert!(!state.select_miller_rows(0, &VPath::from(dir.path()), Selection::new(), Some(99)));
    assert_eq!(state.miller_columns[0].selected_row, Some(0));
    assert_eq!(state.miller_revision, 0);
}

#[test]
fn reselecting_an_open_ancestor_preserves_the_nested_branch() {
    let fixture = tempfile::tempdir().unwrap();
    let child = fixture.path().join("Child");
    let nested = child.join("Nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("note.txt"), b"note").unwrap();
    let mut state = PaneState::from_session(&PaneSession::default(), VPath::from(fixture.path()));
    state.view_mode = PaneViewMode::Columns;
    state.miller_columns = vec![column(fixture.path()), column(&child), column(&nested)];
    let leaf = state.miller_columns[2].listing.as_ref().unwrap();
    state
        .selection
        .select(SelectionKey::for_entry(leaf.parent(), leaf.row(0).unwrap()));
    let cancel = CancelToken::new();
    state.miller_cancel = Some(cancel.clone());
    let root = state.miller_columns[0].listing.as_ref().unwrap();
    let mut ancestor = Selection::new();
    ancestor.select(SelectionKey::for_entry(root.parent(), root.row(0).unwrap()));
    assert!(state.select_miller_rows(0, &VPath::from(fixture.path()), ancestor.clone(), Some(0)));
    assert_eq!(state.miller_columns.len(), 3);
    assert_eq!(state.miller_columns[2].path, VPath::from(nested.as_path()));
    assert_eq!(state.selection, ancestor);
    assert_eq!(
        state.selected_miller_sources(),
        vec![VPath::from(child.as_path())]
    );
    assert_eq!(
        pane_view::selected_drag_paths(&state),
        vec![VPath::from(child.as_path())]
    );
    assert!(!cancel.is_cancelled());
}

#[test]
fn resizing_has_usable_bounds() {
    assert_eq!(pane_view::miller_column_width(280, 50.0), 330);
    assert_eq!(pane_view::miller_column_width(280, -900.0), 220);
    assert_eq!(pane_view::miller_column_width(280, 900.0), 600);
}

#[test]
#[ignore = "requires an isolated GTK display/session; run alone with --ignored --test-threads=1"]
fn gtk_miller_navigation_selection_resize_and_focus() {
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let albums = fixture.path().join("Albums");
    std::fs::create_dir(&albums).unwrap();
    std::fs::create_dir(fixture.path().join("Empty folder")).unwrap();
    let unavailable = fixture.path().join("Unavailable folder");
    std::fs::create_dir(&unavailable).unwrap();
    for name in [
        "Landscape photography collection.txt",
        "Notes.txt",
        "Trip itinerary.pdf",
    ] {
        std::fs::write(albums.join(name), "Miller view fixture").unwrap();
    }
    std::fs::write(fixture.path().join("README.txt"), "Miller view fixture").unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let mut session = SessionState {
        dual_pane: false,
        sidebar_visible: false,
        preview_visible: false,
        appearance: AppearanceMode::Dark,
        ..SessionState::default()
    };
    session.left.view_mode = PaneViewMode::Columns;
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
    // Exercise GDK's advertised clipboard payloads without replacing the user's clipboard.
    let provider = clipboard::provider(&[VPath::from("/tmp/a b\nline")], true).unwrap();
    for (mime, expected) in [
        (
            "x-special/gnome-copied-files",
            "cut\nfile:///tmp/a%20b%0Aline",
        ),
        ("text/uri-list", "file:///tmp/a%20b%0Aline\r\n"),
        ("application/x-kde-cutselection", "1"),
    ] {
        let stream = gio::MemoryOutputStream::new_resizable();
        glib::MainContext::default()
            .block_on(provider.write_mime_type_future(mime, &stream, glib::Priority::DEFAULT))
            .unwrap();
        stream.close(gio::Cancellable::NONE).unwrap();
        assert_eq!(stream.steal_as_bytes().as_ref(), expected.as_bytes());
    }

    wait_until(|| {
        app.model()
            .pane(PaneId::Left)
            .miller_columns
            .first()
            .is_some_and(|column| column.listing.is_some() && !column.loading)
    });
    assert!(!app.widgets().topbar.back.is_sensitive());
    assert!(!app.widgets().topbar.forward.is_sensitive());
    assert!(app.widgets().topbar.up.is_sensitive());
    let album_row = row_for(&app.model(), 0, "Albums");
    app.emit(AppMsg::MillerOpen(PaneId::Left, 0, album_row));
    // A listing arriving after the user focuses the filter must not take focus back.
    app.widgets().global_search.grab_focus();
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns.len() == 2
            && app.model().pane(PaneId::Left).miller_columns[1]
                .listing
                .is_some()
    });
    let focus = gtk::prelude::GtkWindowExt::focus(app.widget()).unwrap();
    assert!(focus.is::<gtk::Text>() || focus.is::<gtk::SearchEntry>());
    assert_eq!(app.widgets().topbar.title.text(), "Albums");
    assert!(app.widgets().topbar.view_buttons[2].is_active());
    // All controls remain reachable with a sidebar or a narrow tiled window.
    // Moving the filter between rows must preserve its keyboard focus.
    for width in [414, 664, 1200, 414] {
        let widgets = app.widgets();
        widgets.topbar.apply_layout(width);
        assert!(widgets.global_search.parent().unwrap().is_visible());
        assert!(widgets.topbar.new_menu.is_visible());
        assert!(widgets.topbar.view_menu.is_visible());
        assert!(widgets.topbar.layout_menu.is_visible());
        assert!(widgets.topbar.more_menu.is_visible());
        let minimum = widgets
            .topbar
            .root
            .measure(gtk::Orientation::Horizontal, -1)
            .0;
        assert!(minimum <= width, "top bar needs {minimum}px at {width}px");
        let focus = gtk::prelude::GtkWindowExt::focus(app.widget()).unwrap();
        assert!(focus.is_ancestor(&widgets.global_search));
    }
    check_centered_topbar(&app);
    // Exercise GTK's own row-hover controller: moving over a sibling in an
    // ancestor column must not change the open branch or its selection.
    let parent_view = app.widgets().panes[0].miller_columns[0]
        .view
        .clone()
        .unwrap();
    let parent_selection = app.widgets().panes[0].miller_columns[0]
        .model
        .as_ref()
        .unwrap()
        .stable_selection();
    hover_miller_row(
        &parent_view,
        &VPath::from(fixture.path().join("Empty folder")),
    );
    drain_frames();
    assert_eq!(
        app.model().pane(PaneId::Left).miller_columns.len(),
        2,
        "hovering a parent folder must preserve its child column"
    );
    assert_eq!(
        app.model().pane(PaneId::Left).current_directory(),
        &VPath::from(albums.as_path())
    );
    assert_eq!(
        app.widgets().panes[0].miller_columns[0]
            .model
            .as_ref()
            .unwrap()
            .stable_selection(),
        parent_selection
    );
    hover_miller_row(
        &parent_view,
        &VPath::from(fixture.path().join("README.txt")),
    );
    drain_frames();
    assert_eq!(app.model().pane(PaneId::Left).miller_columns.len(), 2);
    assert!(!parent_view.is_single_click_activate());
    let generation = app.model().pane(PaneId::Left).miller_generation;
    // A native row grabs focus on press; pane activation is processed before
    // release. It must not move focus (and horizontal scroll) to the last column.
    let album_item = miller_row_widget(&parent_view, &VPath::from(albums.as_path()))
        .parent()
        .unwrap();
    assert!(album_item.grab_focus());
    app.emit(AppMsg::ActivatePane(PaneId::Left));
    drain_frames();
    assert_eq!(
        gtk::prelude::GtkWindowExt::focus(app.widget()),
        Some(album_item),
        "pressing an ancestor must keep focus on the clicked row"
    );
    click_miller_folder(&app, 0, "Albums");
    drain_frames();
    assert_eq!(app.model().pane(PaneId::Left).miller_generation, generation);
    assert_eq!(app.model().pane(PaneId::Left).miller_columns.len(), 2);
    click_miller_folder(&app, 0, "Empty folder");
    wait_until(|| {
        app.model().pane(PaneId::Left).current_directory()
            == &VPath::from(fixture.path().join("Empty folder"))
            && !app.model().pane(PaneId::Left).loading
            && app.model().pane(PaneId::Left).miller_columns[1]
                .listing
                .is_some()
    });
    drain_frames();
    assert_miller_focus(&app, 0, "Empty folder");
    let generation = app.model().pane(PaneId::Left).miller_generation;
    app.emit(AppMsg::Refresh(PaneId::Left));
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_generation > generation
            && !app.model().pane(PaneId::Left).loading
    });
    drain_frames();
    assert_miller_focus(&app, 0, "Empty folder");
    click_miller_folder(&app, 0, "Albums");
    wait_until(|| {
        app.model().pane(PaneId::Left).current_directory() == &VPath::from(albums.as_path())
            && !app.model().pane(PaneId::Left).loading
    });
    let model = app.widgets().panes[0].miller_columns[1]
        .model
        .clone()
        .unwrap();
    assert!(
        model.stable_selection().is_empty(),
        "the child model must not inherit its parent's selected folder"
    );
    select_miller_item(&app, 1, "Notes.txt", true, false);
    wait_until(|| {
        app.model().operation_sources(PaneId::Left) == vec![VPath::from(albums.join("Notes.txt"))]
    });
    let generation = app.model().pane(PaneId::Left).miller_generation;
    app.emit(AppMsg::Refresh(PaneId::Left));
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_generation > generation
            && !app.model().pane(PaneId::Left).loading
    });
    drain_frames();
    assert_miller_focus(&app, 1, "Notes.txt");
    // Background changes can insert rows before the focused file. Track its
    // identity, rather than restoring the previous numeric row position.
    let inserted = albums.join("A newly inserted.txt");
    std::fs::write(&inserted, b"watch fixture").unwrap();
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns[1]
            .listing
            .as_ref()
            .is_some_and(|listing| listing.len() == 4)
    });
    drain_frames();
    assert_miller_focus(&app, 1, "Notes.txt");
    assert_eq!(
        app.model().operation_sources(PaneId::Left),
        vec![VPath::from(albums.join("Notes.txt"))]
    );
    std::fs::remove_file(&inserted).unwrap();
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns[1]
            .listing
            .as_ref()
            .is_some_and(|listing| listing.len() == 3)
    });
    drain_frames();
    assert_miller_focus(&app, 1, "Notes.txt");
    assert!(
        app.widgets().panes[0].miller_columns[0]
            .model
            .as_ref()
            .unwrap()
            .stable_selection()
            .is_empty()
    );
    select_miller_item(&app, 1, "Notes.txt", true, false);
    drain_frames();
    assert!(app.model().pane(PaneId::Left).selection.is_empty());
    assert!(
        model.selection().is_empty(),
        "deselection must survive a redraw"
    );
    select_miller_item(&app, 1, "Trip itinerary.pdf", false, false);
    select_miller_item(&app, 1, "Landscape photography collection.txt", false, true);
    drain_frames();
    assert_eq!(model.selection().size(), 3);
    assert_eq!(
        app.model().pane(PaneId::Left).miller_columns[1].selected_row,
        Some(0)
    );
    assert_eq!(
        gtk::prelude::GtkWindowExt::focus(app.widget()),
        Some(
            miller_row_widget(
                &app.widgets().panes[0].miller_columns[1]
                    .view
                    .clone()
                    .unwrap(),
                &VPath::from(albums.join("Landscape photography collection.txt")),
            )
            .parent()
            .unwrap()
        ),
        "an upward Shift-click must keep focus at the clicked end of the range"
    );
    // Repeating a supported range operation must not trigger GTK's fallback
    // toggle, and shrinking the range must clear old row highlights.
    select_miller_item(&app, 1, "Landscape photography collection.txt", true, true);
    assert_eq!(model.selection().size(), 3);
    select_miller_item(&app, 1, "Notes.txt", false, true);
    drain_frames();
    assert_eq!(model.selection().size(), 2);
    let first_item = miller_row_widget(
        &app.widgets().panes[0].miller_columns[1]
            .view
            .clone()
            .unwrap(),
        &VPath::from(albums.join("Landscape photography collection.txt")),
    )
    .parent()
    .unwrap();
    assert!(!first_item.state_flags().contains(gtk::StateFlags::SELECTED));
    model.select_item(0, true);
    model.select_item(1, false);
    wait_until(|| app.model().pane(PaneId::Left).selection.len() == 2);
    assert_eq!(app.model().operation_sources(PaneId::Left).len(), 2);
    let third_file = VPath::from(albums.join("Trip itinerary.pdf"));
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((third_file.clone(), EntryKind::File)),
    ));
    wait_until(|| app.model().operation_sources(PaneId::Left) == vec![third_file.clone()]);
    model.select_item(0, true);
    model.select_item(1, false);
    wait_until(|| app.model().pane(PaneId::Left).selection.len() == 2);
    let column_before = app.widgets().panes[0].miller_columns[1].container.clone();
    app.emit(AppMsg::MillerResize(
        PaneId::Left,
        VPath::from(albums.as_path()),
        220,
    ));
    wait_until(|| app.model().pane(PaneId::Left).miller_columns[1].width == 220);
    drain_frames();
    assert!(
        column_before.width() <= 230,
        "long names must not force a wider column: {}",
        column_before.width()
    );
    app.emit(AppMsg::MillerResize(
        PaneId::Left,
        VPath::from(albums.as_path()),
        420,
    ));
    wait_until(|| app.model().pane(PaneId::Left).miller_columns[1].width == 420);
    assert_eq!(
        app.widgets().panes[0].miller_columns[1].container,
        column_before
    );
    drain_frames();
    assert!(
        column_before.width() >= 410 && column_before.width() <= 430,
        "column must use requested width: {}",
        column_before.width()
    );
    snapshot(app.widget(), "miller-dark");
    apply_appearance(AppearanceMode::Light);
    drain_frames();
    snapshot(app.widget(), "miller-light");
    apply_appearance(AppearanceMode::Dark);
    // Creation, filtering and refreshed child snapshots all use the visible folder.
    app.emit(AppMsg::CreateFile("created-in-column.txt".to_owned()));
    wait_until(|| {
        albums.join("created-in-column.txt").exists()
            && app.model().pane(PaneId::Left).miller_columns[1]
                .listing
                .as_ref()
                .is_some_and(|listing| {
                    listing
                        .rows()
                        .any(|entry| entry.name() == "created-in-column.txt")
                })
    });
    assert!(!fixture.path().join("created-in-column.txt").exists());
    app.emit(AppMsg::CreateDirectory("Created folder".to_owned()));
    wait_until(|| {
        albums.join("Created folder").is_dir() && !app.model().pane(PaneId::Left).loading
    });
    assert!(!fixture.path().join("Created folder").exists());
    std::fs::write(albums.join(".hidden-note"), b"hidden").unwrap();
    app.emit(AppMsg::Refresh(PaneId::Left));
    wait_until(|| !app.model().pane(PaneId::Left).loading);
    app.emit(AppMsg::ToggleHiddenActive);
    wait_until(|| {
        !app.model().pane(PaneId::Left).filtering
            && app.model().pane(PaneId::Left).miller_columns[1]
                .listing
                .as_ref()
                .is_some_and(|listing| listing.rows().any(|entry| entry.name() == ".hidden-note"))
    });
    app.widgets().global_search.grab_focus();
    app.widgets().global_search.set_text("Notes");
    wait_until(|| {
        app.model().pane(PaneId::Left).filter_query == "Notes"
            && !app.model().pane(PaneId::Left).filtering
    });
    assert_eq!(app.model().pane(PaneId::Left).miller_columns.len(), 2);
    assert_eq!(
        app.model().pane(PaneId::Left).miller_columns[1]
            .listing
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    assert!(
        app.model().pane(PaneId::Left).miller_columns[0]
            .listing
            .as_ref()
            .unwrap()
            .len()
            > 1
    );
    assert!(app.widgets().global_search.has_css_class("has-filter"));
    app.widgets().topbar.apply_layout(1200);
    assert_eq!(app.widgets().global_search.text(), "Notes");
    app.widgets().topbar.apply_layout(414);
    assert_eq!(app.widgets().global_search.text(), "Notes");
    app.widgets()
        .global_search
        .emit_by_name::<()>("stop-search", &[]);
    wait_until(|| {
        app.model().pane(PaneId::Left).filter_query.is_empty()
            && !app.model().pane(PaneId::Left).filtering
    });
    // An async paste retains its captured destination even after navigation changes.
    let clipboard_source = tempfile::tempdir().unwrap();
    let incoming = clipboard_source.path().join("incoming.txt");
    std::fs::write(&incoming, b"clipboard").unwrap();
    app.emit(AppMsg::ClipboardFiles {
        pane: PaneId::Left,
        destination: VPath::from(albums.as_path()),
        result: Ok((vec![VPath::from(incoming.as_path())], false)),
        owner: None,
    });
    wait_until(|| albums.join("incoming.txt").exists() && app.model().active_operations == 0);
    assert!(!fixture.path().join("incoming.txt").exists());
    // Archive creation goes through the job list even with the inspector hidden,
    // and publishes into the current Miller column.
    let notes = VPath::from(albums.join("Notes.txt"));
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((notes, EntryKind::File)),
    ));
    app.emit(AppMsg::CreateArchive {
        name: "column-backup".to_owned(),
        format: ArchiveFormat::TarGz,
    });
    wait_until(|| {
        app.model()
            .operations
            .values()
            .any(|job| job.kind == OperationKind::CreateArchive && job.state == JobState::Done)
    });
    assert!(app.widgets().jobs.root.is_visible());
    assert!(!app.model().preview_visible);
    let archive = albums.join("column-backup.tar.gz");
    assert!(archive.is_file());
    assert!(!fixture.path().join("column-backup.tar.gz").exists());
    assert!(
        app.model()
            .operations
            .values()
            .any(|job| job.kind == OperationKind::CreateArchive
                && job.progress.bytes_done > 0
                && job.progress.items_done == 1)
    );
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((VPath::from(archive.as_path()), EntryKind::File)),
    ));
    app.emit(AppMsg::ExecuteCommand(CommandId::ExtractArchive));
    wait_until(|| {
        app.model()
            .operations
            .values()
            .any(|job| job.kind == OperationKind::ExtractArchive && job.state == JobState::Done)
    });
    assert_eq!(
        std::fs::read(fixture.path().join("Notes.txt")).unwrap(),
        b"Miller view fixture"
    );
    assert_eq!(app.model().active_operations, 0);
    // Per-folder state restores on returning from a different root.
    app.emit(AppMsg::Navigate(
        PaneId::Left,
        VPath::from(clipboard_source.path()),
    ));
    wait_until(|| {
        app.model().pane(PaneId::Left).active().path == VPath::from(clipboard_source.path())
            && !app.model().pane(PaneId::Left).loading
    });
    assert!(app.widgets().topbar.back.is_sensitive());
    assert!(!app.widgets().topbar.forward.is_sensitive());
    app.widgets().topbar.back.emit_clicked();
    wait_until(|| {
        app.model().pane(PaneId::Left).current_directory() == &VPath::from(albums.as_path())
            && !app.model().pane(PaneId::Left).loading
    });
    assert!(app.widgets().topbar.forward.is_sensitive());
    assert_eq!(app.model().pane(PaneId::Left).miller_columns[1].width, 420);
    app.emit(AppMsg::MoveCursorHorizontal(-1, false));
    wait_until(|| app.model().pane(PaneId::Left).miller_columns.len() == 1);
    assert!(app.model().pane(PaneId::Left).selection.is_empty());
    let empty_row = row_for(&app.model(), 0, "Empty folder");
    app.emit(AppMsg::MillerOpen(PaneId::Left, 0, empty_row));
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns.len() == 2
            && app.model().pane(PaneId::Left).miller_columns[1]
                .listing
                .is_some()
    });
    // The empty folder itself remains selected and can be archived. Clicking
    // its blank contents clears that selection without selecting an ancestor.
    assert_eq!(
        app.model().operation_sources(PaneId::Left),
        vec![VPath::from(fixture.path().join("Empty folder"))]
    );
    app.emit(AppMsg::ContextTarget(PaneId::Left, None));
    wait_until(|| app.model().pane(PaneId::Left).selection.is_empty());
    assert!(app.model().focused_item(PaneId::Left).is_none());
    assert!(app.model().operation_sources(PaneId::Left).is_empty());
    assert_eq!(app.widgets().panes[0].status.text(), "0 items, 0 selected");
    assert!(
        app.widgets().panes[0].miller_columns[1]
            .placeholder
            .is_visible()
    );
    drain_frames();
    snapshot(app.widget(), "miller-empty");
    let generation = app.model().pane(PaneId::Left).generation;
    app.emit(AppMsg::Refresh(PaneId::Left));
    wait_until(|| {
        app.model().pane(PaneId::Left).generation > generation
            && !app.model().pane(PaneId::Left).loading
    });
    assert!(
        app.model().focused_item(PaneId::Left).is_none(),
        "refreshing an empty child must not focus an ancestor item"
    );
    app.emit(AppMsg::MoveCursorHorizontal(-1, false));
    wait_until(|| app.model().pane(PaneId::Left).miller_columns.len() == 1);
    let unavailable_row = row_for(&app.model(), 0, "Unavailable folder");
    std::fs::remove_dir(&unavailable).unwrap();
    app.emit(AppMsg::MillerOpen(PaneId::Left, 0, unavailable_row));
    wait_until(|| {
        app.model()
            .pane(PaneId::Left)
            .miller_columns
            .get(1)
            .is_some_and(|column| column.error.is_some())
    });
    assert!(
        app.model()
            .operation_sources(PaneId::Left)
            .iter()
            .all(|path| path == &VPath::from(unavailable.as_path()))
    );
    app.emit(AppMsg::MillerResize(
        PaneId::Left,
        VPath::from(unavailable.as_path()),
        360,
    ));
    wait_until(|| app.model().pane(PaneId::Left).miller_columns[1].width == 360);
    snapshot(app.widget(), "miller-error");
    std::fs::create_dir(&unavailable).unwrap();
    app.emit(AppMsg::MillerRetry(
        PaneId::Left,
        1,
        VPath::from(unavailable.as_path()),
    ));
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns[1]
            .listing
            .is_some()
    });
    assert_eq!(app.model().pane(PaneId::Left).miller_columns[1].width, 360);
    app.emit(AppMsg::MillerResize(
        PaneId::Left,
        VPath::from(unavailable.as_path()),
        280,
    ));
    wait_until(|| app.model().pane(PaneId::Left).miller_columns[1].width == 280);
    app.emit(AppMsg::Refresh(PaneId::Left));
    app.widgets().topbar.view_buttons[0].emit_clicked();
    wait_until(|| {
        app.model().pane(PaneId::Left).view_mode == PaneViewMode::List
            && !app.model().pane(PaneId::Left).loading
    });
    assert_eq!(
        app.model().pane(PaneId::Left).current_directory(),
        &VPath::from(unavailable.as_path())
    );
    assert!(app.widgets().topbar.view_buttons[0].is_active());
    let menus = {
        let widgets = app.widgets();
        [
            (widgets.topbar.new_menu.clone(), "topbar-new"),
            (widgets.topbar.view_menu.clone(), "topbar-view"),
            (widgets.topbar.layout_menu.clone(), "topbar-layout"),
            (widgets.topbar.more_menu.clone(), "topbar-more"),
        ]
    };
    for (button, name) in menus {
        button.popup();
        drain_frames();
        snapshot_popover(app.widget(), &button.popover().unwrap(), name);
        button.popdown();
        drain_frames();
    }
    app.emit(AppMsg::ToggleDualPane);
    app.emit(AppMsg::ActivatePane(PaneId::Right));
    wait_until(|| app.model().active_pane == PaneId::Right);
    assert_eq!(
        app.widgets().global_search.placeholder_text().as_deref(),
        Some("Filter Right pane…")
    );
    app.widgets().global_search.set_text("README");
    wait_until(|| app.model().pane(PaneId::Right).filter_query == "README");
    app.emit(AppMsg::ActivatePane(PaneId::Left));
    wait_until(|| app.model().active_pane == PaneId::Left);
    assert_eq!(app.widgets().global_search.text(), "");
    assert_eq!(
        app.widgets().global_search.placeholder_text().as_deref(),
        Some("Filter Left pane…")
    );
    assert!(!app.widgets().global_search.has_css_class("has-filter"));
    assert_eq!(app.widgets().topbar.title.text(), "Unavailable folder");
    app.emit(AppMsg::ExecuteCommand(CommandId::ToggleSidebar));
    wait_until(|| app.model().sidebar_visible);
    drain_frames();
    assert!(app.widgets().global_search.is_mapped());
    assert!(
        app.widgets()
            .topbar
            .root
            .measure(gtk::Orientation::Horizontal, -1)
            .0
            <= app.widget().width() - SIDEBAR_WIDTH
    );
    snapshot(app.widget(), "topbar-sidebar");
    app.emit(AppMsg::ExecuteCommand(CommandId::ToggleSidebar));
    app.emit(AppMsg::ToggleDualPane);
    wait_until(|| !app.model().sidebar_visible && !app.model().dual_pane);
    app.widgets().topbar.more_menu.popup();
    drain_frames();
    app.widgets().topbar.recovery.emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_some());
    drain_frames();
    snapshot(app.widget(), "recovery");
    app.widget().visible_dialog().unwrap().close();
    drain_frames();
    if std::env::var_os("COMMANDER_STYLE_SNAPSHOTS").is_some() {
        style_gallery(&app, fixture.path());
    }
    check_navigation_keeps_view_mode(&app, fixture.path());
    app.widget().close();
}

fn check_navigation_keeps_view_mode(app: &relm4::Controller<AppModel>, fixture: &std::path::Path) {
    let first = fixture.join("View mode first");
    let second = fixture.join("View mode second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let first = VPath::from(first);
    let second = VPath::from(second);
    let ready = |path: &VPath| {
        let model = app.model();
        let pane = model.pane(PaneId::Left);
        !pane.loading && pane.current_directory() == path
    };
    for (mode, button) in [(PaneViewMode::List, 0), (PaneViewMode::Grid, 1)] {
        // Leave the first folder in Columns, then choose a different view elsewhere.
        app.emit(AppMsg::Navigate(PaneId::Left, first.clone()));
        app.emit(AppMsg::SetViewMode(PaneViewMode::Columns));
        wait_until(|| ready(&first));
        app.emit(AppMsg::Navigate(PaneId::Left, second.clone()));
        wait_until(|| ready(&second));
        app.emit(AppMsg::SetViewMode(mode));
        wait_until(|| app.model().pane(PaneId::Left).view_mode == mode);
        for (message, path) in [
            (AppMsg::Navigate(PaneId::Left, first.clone()), &first),
            (AppMsg::Back(PaneId::Left), &second),
            (AppMsg::Forward(PaneId::Left), &first),
        ] {
            app.emit(message);
            wait_until(|| ready(path));
            assert_eq!(app.model().pane(PaneId::Left).view_mode, mode);
            assert!(app.widgets().topbar.view_buttons[button].is_active());
        }
    }
}

/// Reuse the native fixture to inspect every main surface after a styling change.
fn style_gallery(app: &relm4::Controller<AppModel>, fixture: &std::path::Path) {
    let workspace = fixture.join("Workspace");
    for name in ["Design", "Documents", "Projects"] {
        std::fs::create_dir_all(workspace.join(name)).unwrap();
    }
    for (name, contents) in [
        (
            "Release notes.md",
            "# Release notes\n\nA clear workspace for everyday files.\n\n- Updated navigation\n- File previews\n- Background operations\n",
        ),
        ("Budget.csv", "Item,Amount\nDesign,1200\nDevelopment,3400\n"),
        (
            "Settings.json",
            "{\n  \"theme\": \"system\",\n  \"view\": \"list\"\n}\n",
        ),
        (
            "main.rs",
            "fn main() {\n    println!(\"Hello, Commander!\");\n}\n",
        ),
        (
            "Meeting notes.txt",
            "Project review\n\nConfirm the release checklist and organize design files.\n",
        ),
    ] {
        std::fs::write(workspace.join(name), contents).unwrap();
    }
    app.widget().fullscreen();
    app.emit(AppMsg::Navigate(
        PaneId::Left,
        VPath::from(workspace.as_path()),
    ));
    app.emit(AppMsg::SetViewMode(PaneViewMode::List));
    app.emit(AppMsg::ExecuteCommand(CommandId::ToggleSidebar));
    wait_until(|| {
        !app.model().pane(PaneId::Left).loading
            && app.model().pane(PaneId::Left).current_directory()
                == &VPath::from(workspace.as_path())
    });
    drain_frames();
    for (appearance, name) in [
        (AppearanceMode::Dark, "clean-list-dark"),
        (AppearanceMode::Light, "clean-list-light"),
    ] {
        apply_appearance(appearance);
        drain_frames();
        snapshot(app.widget(), name);
    }
    app.emit(AppMsg::SetViewMode(PaneViewMode::Grid));
    wait_until(|| app.model().pane(PaneId::Left).view_mode == PaneViewMode::Grid);
    drain_frames();
    snapshot(app.widget(), "clean-grid-light");
    apply_appearance(AppearanceMode::Dark);
    drain_frames();
    snapshot(app.widget(), "clean-grid-dark");
    app.emit(AppMsg::SetViewMode(PaneViewMode::List));
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((
            VPath::from(workspace.join("Release notes.md")),
            EntryKind::File,
        )),
    ));
    app.emit(AppMsg::ExecuteCommand(CommandId::TogglePreview));
    wait_until(|| app.model().preview_visible && !app.model().preview_state.loading);
    drain_frames();
    snapshot(app.widget(), "clean-inspector-dark");
    apply_appearance(AppearanceMode::Light);
    drain_frames();
    snapshot(app.widget(), "clean-inspector-light");
    // Render the real file menu, including its filter, shortcuts, and scroll area.
    let view = app.widgets().panes[0].column_view.clone();
    let target = VPath::from(workspace.join("Release notes.md"));
    let y = (0..view.height())
        .find(|y| {
            context_menu::context_target_at(view.upcast_ref(), 60.0, f64::from(*y))
                .is_some_and(|(path, _)| path == target)
        })
        .expect("visible context-menu target");
    let controllers = view.observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(gesture) = controllers.item(index).and_downcast::<gtk::GestureClick>()
            && gesture.button() == 3
        {
            gesture.emit_by_name::<()>("pressed", &[&1_i32, &60.0_f64, &f64::from(y)]);
        }
    }
    drain_frames();
    let mut child = view.first_child();
    let menu = loop {
        let widget = child.expect("open file context menu");
        child = widget.next_sibling();
        if let Ok(menu) = widget.downcast::<gtk::Popover>() {
            break menu;
        }
    };
    for (appearance, name) in [
        (AppearanceMode::Dark, "context-menu-dark"),
        (AppearanceMode::Light, "context-menu-light"),
    ] {
        apply_appearance(appearance);
        drain_frames();
        snapshot_popover(app.widget(), &menu, name);
    }
    menu.popdown();
    drain_frames();
    // Include short and overflowing code in the optional native screenshot gallery.
    for (contents, length) in [
        (
            "fn main() {\n    println!(\"Hello, Commander!\");\n}\n".to_owned(),
            "short",
        ),
        (
            (0..100)
                .map(|line| format!("let value_{line} = \"{}\";\n", "long code line ".repeat(12)))
                .collect(),
            "long",
        ),
    ] {
        let path = workspace.join(format!("quick-look-{length}.rs"));
        std::fs::write(&path, contents).unwrap();
        app.emit(AppMsg::ExecuteCommand(CommandId::Refresh));
        wait_until(|| {
            let model = app.model();
            let pane = model.pane(PaneId::Left);
            !pane.loading
                && pane.active().listing.as_ref().is_some_and(|listing| {
                    listing
                        .rows()
                        .any(|entry| Some(entry.name()) == path.file_name())
                })
        });
        app.emit(AppMsg::ContextTarget(
            PaneId::Left,
            Some((VPath::from(path.as_path()), EntryKind::File)),
        ));
        wait_until(|| {
            let model = app.model();
            !model.preview_state.loading
                && model.preview_state.path.as_ref() == Some(&VPath::from(path.as_path()))
        });
        app.emit(AppMsg::ToggleQuickLook);
        wait_until(|| app.widget().visible_dialog().is_some());
        for (appearance, theme) in [
            (AppearanceMode::Dark, "dark"),
            (AppearanceMode::Light, "light"),
        ] {
            apply_appearance(appearance);
            drain_frames();
            snapshot(app.widget(), &format!("quick-look-{length}-{theme}"));
        }
        app.emit(AppMsg::ToggleQuickLook);
        wait_until(|| app.widget().visible_dialog().is_none());
    }
    app.emit(AppMsg::ExecuteCommand(CommandId::CommandPalette));
    wait_until(|| app.model().palette_open);
    drain_frames();
    snapshot(app.widget(), "clean-palette-light");
    app.emit(AppMsg::ClosePalette);
    wait_until(|| !app.model().palette_open);
    app.emit(AppMsg::ExecuteCommand(CommandId::TogglePreview));
    for name in ["Design", "Documents"] {
        let count = app.model().pane(PaneId::Left).tabs.len() + 1;
        app.emit(AppMsg::NewTab(PaneId::Left));
        app.emit(AppMsg::Navigate(
            PaneId::Left,
            VPath::from(workspace.join(name)),
        ));
        wait_until(|| {
            let model = app.model();
            let pane = model.pane(PaneId::Left);
            pane.tabs.len() == count
                && !pane.loading
                && pane.current_directory() == &VPath::from(workspace.join(name))
        });
    }
    for (appearance, name) in [
        (AppearanceMode::Dark, "tab-pills-dark"),
        (AppearanceMode::Light, "tab-pills-light"),
    ] {
        apply_appearance(appearance);
        drain_frames();
        snapshot(app.widget(), name);
    }
    app.widget().unfullscreen();
}

fn row_for(model: &AppModel, column: usize, name: &str) -> u32 {
    model.pane(PaneId::Left).miller_columns[column]
        .listing
        .as_ref()
        .unwrap()
        .rows()
        .position(|entry| entry.name() == name)
        .unwrap() as u32
}

fn check_centered_topbar(app: &relm4::Controller<AppModel>) {
    app.widgets().global_search.set_text("Notes");
    wait_until(|| {
        app.model().pane(PaneId::Left).filter_query == "Notes"
            && !app.model().pane(PaneId::Left).filtering
    });
    app.widgets().global_search.select_region(1, 4);
    let mut problems = Vec::new();
    for (width, sidebar) in [
        (1600, false),
        (1200, false),
        (1000, false),
        (900, false),
        (720, false),
        (1200, true),
        (900, true),
        (720, true),
        (1600, true),
        (1520, false),
    ] {
        if app.model().sidebar_visible != sidebar {
            app.emit(AppMsg::ExecuteCommand(CommandId::ToggleSidebar));
        }
        app.widget().set_default_size(width, 900);
        wait_until(|| {
            app.widget().width() == width
                && app.model().window_width == app.widget().surface().unwrap().width()
                && app.model().sidebar_visible == sidebar
        });
        drain_frames();
        // A long title must yield space without displacing the center field.
        app.widgets()
            .topbar
            .title
            .set_label("A long project folder with a descriptive name");
        drain_frames();
        let widgets = app.widgets();
        let bar = &widgets.topbar.root;
        let search = widgets.global_search.compute_bounds(bar).unwrap();
        let offset = (search.x() + search.width() / 2.0 - bar.width() as f32 / 2.0).abs();
        if offset > 1.0 {
            problems.push(format!(
                "search is {offset}px off center at {width}px, sidebar={sidebar}"
            ));
        }
        if search.width() < 180.0 || search.width() > 560.0 {
            problems.push(format!(
                "search width is {}px at {width}px, sidebar={sidebar}",
                search.width()
            ));
        }
        for menu in [
            &widgets.topbar.new_menu,
            &widgets.topbar.view_menu,
            &widgets.topbar.layout_menu,
            &widgets.topbar.more_menu,
        ] {
            let bounds = menu.compute_bounds(bar).unwrap();
            if bounds.x() < 0.0 || bounds.x() + bounds.width() > bar.width() as f32 + 1.0 {
                problems.push(format!(
                    "menu outside top bar at {width}px, sidebar={sidebar}"
                ));
            }
            let same_row = search.y() < bounds.y() + bounds.height()
                && bounds.y() < search.y() + search.height();
            if same_row && search.x() + search.width() > bounds.x() + 1.0 {
                problems.push(format!(
                    "search overlaps menu at {width}px, sidebar={sidebar}"
                ));
            }
        }
        assert!(widgets.global_search.is_mapped());
        assert_eq!(widgets.global_search.text(), "Notes");
        assert_eq!(widgets.global_search.selection_bounds(), Some((1, 4)));
        assert!(
            gtk::prelude::GtkWindowExt::focus(app.widget())
                .unwrap()
                .is_ancestor(&widgets.global_search)
        );
        if let Some(directory) = std::env::var_os("COMMANDER_MILLER_SNAPSHOT_DIR") {
            let snapshot = gtk::Snapshot::new();
            bar.parent().unwrap().snapshot_child(bar, &snapshot);
            let node = snapshot.to_node().unwrap();
            app.widget()
                .renderer()
                .unwrap()
                .render_texture(&node, None)
                .save_to_png(
                    std::path::Path::new(&directory)
                        .join(format!("centered-search-{width}-{sidebar}.png")),
                )
                .unwrap();
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    app.widgets().global_search.set_text("");
    wait_until(|| {
        app.model().pane(PaneId::Left).filter_query.is_empty()
            && !app.model().pane(PaneId::Left).filtering
            && app.model().pane(PaneId::Left).miller_columns[1]
                .listing
                .as_ref()
                .unwrap()
                .len()
                == 3
    });
}

fn miller_row_widget(view: &gtk::ListView, path: &VPath) -> gtk::Widget {
    fn find(widget: &gtk::Widget, path: &str) -> Option<gtk::Widget> {
        if widget.has_css_class("column-browser-row")
            && widget.tooltip_text().as_deref() == Some(path)
        {
            return Some(widget.clone());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if let Some(found) = find(&widget, path) {
                return Some(found);
            }
            child = widget.next_sibling();
        }
        None
    }
    find(view.upcast_ref(), &path.to_string()).expect("bound Miller row")
}

fn hover_miller_row(view: &gtk::ListView, path: &VPath) {
    let row = miller_row_widget(view, path).parent().unwrap();
    let controllers = row.observe_controllers();
    let mut hovered = false;
    for index in 0..controllers.n_items() {
        if let Some(motion) = controllers
            .item(index)
            .and_downcast::<gtk::EventControllerMotion>()
        {
            motion.emit_by_name::<()>("enter", &[&1.0_f64, &1.0_f64]);
            hovered = true;
        }
    }
    assert!(hovered, "GTK's row motion controller must be exercised");
}

fn click_miller_folder(app: &relm4::Controller<AppModel>, column: usize, name: &str) {
    let view = app.widgets().panes[0].miller_columns[column]
        .view
        .clone()
        .unwrap();
    let path = app.model().pane(PaneId::Left).miller_columns[column]
        .path
        .join_name(OsStr::new(name));
    let row = miller_row_widget(&view, &path);
    let bounds = row.compute_bounds(&view).unwrap();
    // GTK selects the row before the view's bubble-phase release handler runs.
    select_miller_item(app, column, name, false, false);
    let controllers = view.observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(gesture) = controllers.item(index).and_downcast::<gtk::GestureClick>()
            && gesture.name().as_deref() == Some("miller-folder-click")
        {
            gesture.emit_by_name::<()>(
                "released",
                &[
                    &1_i32,
                    &f64::from(bounds.x() + bounds.width() / 2.0),
                    &f64::from(bounds.y() + bounds.height() / 2.0),
                ],
            );
            return;
        }
    }
    panic!("Miller folder click controller is installed");
}

fn select_miller_item(
    app: &relm4::Controller<AppModel>,
    column: usize,
    name: &str,
    modify: bool,
    extend: bool,
) {
    let view = app.widgets().panes[0].miller_columns[column]
        .view
        .clone()
        .unwrap();
    let path = app.model().pane(PaneId::Left).miller_columns[column]
        .path
        .join_name(OsStr::new(name));
    let item = miller_row_widget(&view, &path).parent().unwrap();
    assert!(item.grab_focus());
    // Use the same GTK action as its native click-release handler, including
    // GTK's range anchor and fallback behavior.
    item.activate_action("listitem.select", Some(&(modify, extend).to_variant()))
        .unwrap();
    drain_frames();
}

#[track_caller]
fn assert_miller_focus(app: &relm4::Controller<AppModel>, column: usize, name: &str) {
    let view = app.widgets().panes[0].miller_columns[column]
        .view
        .clone()
        .unwrap();
    let path = app.model().pane(PaneId::Left).miller_columns[column]
        .path
        .join_name(OsStr::new(name));
    assert_eq!(
        gtk::prelude::GtkWindowExt::focus(app.widget()),
        miller_row_widget(&view, &path).parent(),
        "the focus outline must stay on {name} when listings refresh"
    );
}

#[track_caller]
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !predicate() {
        assert!(
            Instant::now() < until,
            "timed out waiting for Miller view update at {}",
            std::panic::Location::caller()
        );
        drain_frames();
    }
}

fn drain_frames() {
    let context = glib::MainContext::default();
    let until = Instant::now() + Duration::from_millis(100);
    while Instant::now() < until {
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn snapshot_popover(window: &adw::ApplicationWindow, popover: &gtk::Popover, name: &str) {
    let Some(directory) = std::env::var_os("COMMANDER_MILLER_SNAPSHOT_DIR") else {
        return;
    };
    let snapshot = gtk::Snapshot::new();
    // Include the contents surface so light-menu screenshots are not transparent.
    let child = popover.child().unwrap().parent().unwrap();
    child.parent().unwrap().snapshot_child(&child, &snapshot);
    let node = snapshot.to_node().unwrap();
    window
        .renderer()
        .unwrap()
        .render_texture(&node, None)
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}

fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    let Some(directory) = std::env::var_os("COMMANDER_MILLER_SNAPSHOT_DIR") else {
        return;
    };
    let widget = gtk::prelude::GtkWindowExt::child(window).unwrap();
    let snapshot = gtk::Snapshot::new();
    widget.parent().unwrap().snapshot_child(&widget, &snapshot);
    let node = snapshot.to_node().unwrap();
    window
        .renderer()
        .unwrap()
        .render_texture(&node, None)
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}

#[test]
fn navigation_session_restores_branch_widths_and_folder_preferences() {
    let fixture = tempfile::tempdir().unwrap();
    let child = fixture.path().join("Child");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(child.join("note"), b"note").unwrap();
    let mut pane = PaneState::from_session(&PaneSession::default(), VPath::from(fixture.path()));
    pane.view_mode = PaneViewMode::Columns;
    pane.show_hidden = true;
    pane.sort.direction = SortDirection::Descending;
    pane.miller_columns = vec![column(fixture.path()), column(&child)];
    pane.miller_columns[0].width = 350;
    pane.miller_columns[1].width = 490;
    pane.miller_columns[1].scroll_y = 240;
    pane.miller_scroll_x = 180;
    let listing = pane.miller_columns[1].listing.as_ref().unwrap();
    pane.selection
        .select_preserving_anchor(SelectionKey::for_entry(
            listing.parent(),
            listing.row(0).unwrap(),
        ));
    assert_eq!(pane.current_directory(), &VPath::from(child.as_path()));
    let encoded = toml_edit::ser::to_string(&pane.to_session()).unwrap();
    let session = toml_edit::de::from_str(&encoded).unwrap();
    let mut restored = PaneState::from_session(&session, VPath::from("/unused"));
    assert_eq!(restored.current_directory(), &VPath::from(child.as_path()));
    assert_eq!(restored.miller_columns[0].width, 350);
    assert_eq!(restored.miller_columns[1].width, 490);
    assert_eq!(restored.miller_columns[1].scroll_y, 240);
    assert_eq!(restored.miller_scroll_x, 180);
    assert_eq!(restored.restore_names, vec!["note"]);
    assert!(restored.show_hidden);
    assert_eq!(restored.sort.direction, SortDirection::Descending);
    restored.miller_columns[1].listing = Some(Arc::clone(listing));
    restored.restore_selection();
    assert!(restored.selection.contains(&SelectionKey::for_entry(
        listing.parent(),
        listing.row(0).unwrap(),
    )));
    assert!(restored.restore_names.is_empty());
}

#[test]
fn pane_view_mode_takes_priority_over_saved_folder_modes() {
    let modes = [
        PaneViewMode::List,
        PaneViewMode::Grid,
        PaneViewMode::Columns,
    ];
    for mode in modes {
        for saved_mode in modes {
            let session = PaneSession {
                tabs: vec!["/first".to_owned(), "/second".to_owned()],
                view_mode: mode,
                folders: ["/first", "/second"]
                    .into_iter()
                    .map(|path| {
                        (
                            path.to_owned(),
                            FolderViewSession {
                                view_mode: saved_mode,
                                sort_key: PaneSortKey::Size,
                                show_hidden: true,
                                ..FolderViewSession::default()
                            },
                        )
                    })
                    .collect(),
                ..PaneSession::default()
            };
            let mut pane = PaneState::from_session(&session, VPath::from("/unused"));
            assert_eq!(
                pane.view_mode, mode,
                "Startup must honor the pane's saved mode"
            );
            pane.active_tab = 1;
            pane.reset_directory_view();
            assert_eq!(
                pane.view_mode, mode,
                "Tab navigation must retain the current mode"
            );
            assert_eq!(pane.sort.key, SortKey::Size);
            assert!(pane.show_hidden);
            let encoded = toml_edit::ser::to_string(&pane.to_session()).unwrap();
            let saved = toml_edit::de::from_str(&encoded).unwrap();
            let restored = PaneState::from_session(&saved, VPath::from("/unused"));
            assert_eq!(restored.view_mode, mode);
        }
    }
}
