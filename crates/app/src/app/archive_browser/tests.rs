use super::*;
use relm4::{Component, ComponentController};

fn archive(source: &Path, destination: &Path, format: ArchiveFormat) {
    let cancel = CancelToken::new();
    create_archive(
        &LocalFs,
        &[VPath::from(source)],
        &VPath::from(destination),
        format,
        &mut ArchiveTask::new(&cancel),
    )
    .unwrap();
}

#[test]
fn supported_archives_open_nested_folders_reuse_snapshots_and_keep_visible_paths() {
    let fixture = tempfile::tempdir().unwrap();
    let docs = fixture.path().join("Documents");
    std::fs::create_dir_all(docs.join("Empty folder")).unwrap();
    std::fs::write(docs.join("readme.txt"), "archive contents").unwrap();
    for format in [
        ArchiveFormat::Zip,
        ArchiveFormat::SevenZ,
        ArchiveFormat::Tar,
        ArchiveFormat::TarGz,
    ] {
        let source = fixture
            .path()
            .join(format!("My archive.{}", format.extension()));
        archive(&docs, &source, format);
        let target = VPath::from(source.join("Documents"));
        let opened = open_location(&LocalFs, &target, &[], &CancelToken::new()).unwrap();
        assert_eq!(
            std::fs::read_to_string(opened.location.as_path().join("readme.txt")).unwrap(),
            "archive contents"
        );
        assert!(opened.location.as_path().join("Empty folder").is_dir());
        let mut paths = ArchiveLocations::default();
        paths.mounts.push(opened.mount.clone());
        assert_eq!(paths.display(&opened.location), target);
        assert_eq!(paths.resolve(&target), opened.location);
        let root = VPath::from(opened.mount.directory.path());
        assert_eq!(paths.parent(&root), Some(VPath::from(fixture.path())));
        assert_eq!(paths.display(&root), VPath::from(source.as_path()));
        assert_eq!(
            paths.resolve(&VPath::from(source.join(".."))),
            VPath::from(fixture.path())
        );
        let again = open_location(&LocalFs, &target, &paths.mounts, &CancelToken::new()).unwrap();
        assert!(Arc::ptr_eq(&opened.mount, &again.mount));
        let mut session = PaneSession {
            tabs: vec![opened.location.to_string()],
            ..PaneSession::default()
        };
        session.locations.insert(
            root.to_string(),
            NavigationSession {
                columns: vec![root.to_string(), opened.location.to_string()],
                ..NavigationSession::default()
            },
        );
        let saved = paths.session(session);
        assert_eq!(saved.tabs, vec![target.to_string()]);
        assert_eq!(
            saved.locations[&source.to_string_lossy().to_string()].columns,
            vec![source.to_string_lossy().to_string(), target.to_string()]
        );
        // A fresh app can open the persisted archive location without its previous cache.
        let restored = open_location(
            &LocalFs,
            &VPath::from(saved.tabs[0].as_str()),
            &[],
            &CancelToken::new(),
        )
        .unwrap();
        assert_ne!(restored.location, opened.location);
        assert!(restored.location.as_path().join("readme.txt").exists());
    }
}

#[test]
fn nested_archives_parent_to_the_outer_archive_and_corrupt_or_cancelled_openings_fail() {
    let fixture = tempfile::tempdir().unwrap();
    let payload = fixture.path().join("Payload");
    std::fs::create_dir(&payload).unwrap();
    std::fs::write(payload.join("note.txt"), "nested").unwrap();
    let inner = fixture.path().join("nested.zip");
    archive(&payload, &inner, ArchiveFormat::Zip);
    let outer = fixture.path().join("outer.tar.gz");
    archive(&inner, &outer, ArchiveFormat::TarGz);
    let outer_open = open_location(
        &LocalFs,
        &VPath::from(outer.as_path()),
        &[],
        &CancelToken::new(),
    )
    .unwrap();
    let inner_path = outer_open.location.join_name(OsStr::new("nested.zip"));
    let inner_open = open_location(
        &LocalFs,
        &inner_path,
        &[Arc::clone(&outer_open.mount)],
        &CancelToken::new(),
    )
    .unwrap();
    assert!(
        inner_open
            .mount
            .editable
            .as_ref()
            .unwrap_err()
            .contains("Nested")
    );
    let paths = ArchiveLocations {
        mounts: vec![outer_open.mount, inner_open.mount],
        ..ArchiveLocations::default()
    };
    assert_eq!(
        paths.display(&inner_open.location),
        VPath::from(outer.join("nested.zip"))
    );
    assert_eq!(
        paths.parent(&inner_open.location),
        Some(outer_open.location)
    );
    let ancestors = paths.ancestors(&inner_open.location);
    assert!(ancestors.iter().all(|path| {
        !paths
            .display(path)
            .to_string()
            .contains("commander-archive-")
    }));
    let corrupt = fixture.path().join("broken.zip");
    std::fs::write(&corrupt, b"not an archive").unwrap();
    assert!(
        open_location(
            &LocalFs,
            &VPath::from(corrupt.as_path()),
            &[],
            &CancelToken::new()
        )
        .is_err()
    );
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(open_location(&LocalFs, &VPath::from(outer.as_path()), &[], &cancel).is_err());
}

#[track_caller]
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Archive browser timed out at {}",
            std::panic::Location::caller()
        );
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn visible(app: &relm4::Controller<AppModel>, pane: PaneId) -> VPath {
    let model = app.model();
    model
        .archive_mounts
        .display(model.pane(pane).current_directory())
}

fn row(app: &relm4::Controller<AppModel>, name: &str) -> u32 {
    app.model()
        .pane(PaneId::Left)
        .active()
        .listing
        .as_ref()
        .unwrap()
        .rows()
        .position(|entry| entry.name() == name)
        .unwrap() as u32
}

fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
    let Some(directory) = std::env::var_os("COMMANDER_TEST_ARTIFACTS") else {
        return;
    };
    let child = gtk::prelude::GtkWindowExt::child(window).unwrap();
    let snapshot = gtk::Snapshot::new();
    window.snapshot_child(&child, &snapshot);
    window
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(
            Path::new(&directory)
                .join("snapshots")
                .join(format!("{name}.png")),
        )
        .unwrap();
}

fn reveal_inspector_metadata(app: &relm4::Controller<AppModel>) {
    let adjustment = app
        .widgets()
        .preview
        .read_only_value
        .ancestor(gtk::ScrolledWindow::static_type())
        .unwrap()
        .downcast::<gtk::ScrolledWindow>()
        .unwrap()
        .vadjustment();
    wait_until(|| adjustment.upper() - adjustment.page_size() >= 300.0);
    adjustment.set_value(300.0);
    assert!(adjustment.value() > 0.0);
}

fn launch(left: VPath, right: VPath, session: Option<SessionState>) -> relm4::Controller<AppModel> {
    let (session_worker, startup) = SessionWorker::start().unwrap();
    AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(left),
                right: Some(right),
                ..AppOptions::default()
            },
            session_worker,
            session,
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach()
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_archive_browser_navigation_copy_out_nested_archives_and_restore() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let docs = fixture.path().join("Documents");
    let destination = fixture.path().join("Extracted files");
    std::fs::create_dir_all(docs.join("Notes")).unwrap();
    std::fs::create_dir(docs.join("Empty folder")).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(
        docs.join("README.md"),
        "# Project notes\n\nBrowse this archive like a folder.\n",
    )
    .unwrap();
    std::fs::write(
        docs.join("Notes/changelog.txt"),
        "Nested archive contents\n",
    )
    .unwrap();
    archive(
        &docs.join("Notes"),
        &docs.join("nested.zip"),
        ArchiveFormat::Zip,
    );
    let bundle = fixture.path().join("Project bundle.zip");
    archive(&docs, &bundle, ArchiveFormat::Zip);
    let original = std::fs::read(&bundle).unwrap();
    let mut session = SessionState {
        sidebar_visible: false,
        preview_visible: false,
        dual_pane: true,
        window_width: 1320,
        window_height: 800,
        ..SessionState::default()
    };
    session.left.view_mode = PaneViewMode::List;
    session.right.view_mode = PaneViewMode::List;
    let app = launch(
        VPath::from(fixture.path()),
        VPath::from(destination.as_path()),
        Some(session),
    );
    app.widget().present();
    wait_until(|| !app.model().pane(PaneId::Left).loading);
    app.emit(AppMsg::OpenRow(
        PaneId::Left,
        row(&app, "Project bundle.zip"),
    ));
    wait_until(|| {
        visible(&app, PaneId::Left) == VPath::from(bundle.as_path())
            && !app.model().pane(PaneId::Left).loading
    });
    assert_eq!(
        app.widgets().panes[0].path_entry.text(),
        bundle.to_str().unwrap()
    );
    assert!(app.widgets().panes[0].status.text().contains("Archive"));
    assert!(!app.widgets().panes[0].status.text().contains("read-only"));
    apply_appearance(AppearanceMode::Dark);
    snapshot(app.widget(), "archive-root-dark");
    app.emit(AppMsg::OpenRow(PaneId::Left, row(&app, "Documents")));
    let virtual_docs = VPath::from(bundle.join("Documents"));
    wait_until(|| {
        visible(&app, PaneId::Left) == virtual_docs && !app.model().pane(PaneId::Left).loading
    });
    apply_appearance(AppearanceMode::Dark);
    snapshot(app.widget(), "archive-list-dark");

    let native_docs = app.model().pane(PaneId::Left).current_directory().clone();
    let readme = native_docs.join_name(OsStr::new("README.md"));
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((readme.clone(), EntryKind::File)),
    ));
    app.emit(AppMsg::ExecuteCommand(CommandId::Copy));
    wait_until(|| destination.join("README.md").exists() && app.model().active_operations == 0);
    assert_eq!(
        std::fs::read(destination.join("README.md")).unwrap(),
        std::fs::read(docs.join("README.md")).unwrap()
    );
    app.emit(AppMsg::ExecuteCommand(CommandId::TogglePreview));
    let copied_readme = VPath::from(destination.join("README.md"));
    app.emit(AppMsg::ContextTarget(
        PaneId::Right,
        Some((copied_readme.clone(), EntryKind::File)),
    ));
    wait_until(|| {
        app.model().preview_state.path.as_ref() == Some(&copied_readme)
            && !app.model().preview_state.loading
            && app.widgets().preview.read_only_value.text() == "No"
    });
    reveal_inspector_metadata(&app);
    snapshot(app.widget(), "archive-copied-file-inspector-dark");
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((readme.clone(), EntryKind::File)),
    ));
    wait_until(|| {
        app.model().preview_state.path.as_ref() == Some(&readme)
            && !app.model().preview_state.loading
            && app.widgets().preview.read_only_value.text() == "No"
    });
    assert!(
        app.model()
            .preview_state
            .content
            .as_ref()
            .unwrap()
            .metadata
            .mode
            .is_some_and(|mode| mode & 0o222 != 0),
        "editable archives must expose writable entries"
    );
    reveal_inspector_metadata(&app);
    snapshot(app.widget(), "archive-member-inspector-dark");
    app.emit(AppMsg::ExecuteCommand(CommandId::TogglePreview));
    app.emit(AppMsg::ExecuteCommand(CommandId::SecureDelete));
    wait_until(|| app.model().pane(PaneId::Left).error.is_some());
    assert!(readme.as_path().exists());
    app.emit(AppMsg::Refresh(PaneId::Left));
    wait_until(|| {
        !app.model().pane(PaneId::Left).loading && app.model().pane(PaneId::Left).error.is_none()
    });
    app.emit(AppMsg::OpenRow(PaneId::Left, row(&app, "nested.zip")));
    let virtual_nested = VPath::from(bundle.join("Documents/nested.zip"));
    wait_until(|| {
        visible(&app, PaneId::Left) == virtual_nested && !app.model().pane(PaneId::Left).loading
    });
    app.emit(AppMsg::Up(PaneId::Left));
    wait_until(|| {
        visible(&app, PaneId::Left) == virtual_docs && !app.model().pane(PaneId::Left).loading
    });
    app.emit(AppMsg::Back(PaneId::Left));
    wait_until(|| visible(&app, PaneId::Left) == virtual_nested);
    app.emit(AppMsg::Forward(PaneId::Left));
    wait_until(|| visible(&app, PaneId::Left) == virtual_docs);

    app.emit(AppMsg::SetViewMode(PaneViewMode::Grid));
    wait_until(|| app.model().pane(PaneId::Left).view_mode == PaneViewMode::Grid);
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "archive-grid-light");
    app.emit(AppMsg::SetViewMode(PaneViewMode::Columns));
    wait_until(|| {
        app.model().pane(PaneId::Left).view_mode == PaneViewMode::Columns
            && !app.widgets().panes[0].miller_columns.is_empty()
    });
    apply_appearance(AppearanceMode::Dark);
    snapshot(app.widget(), "archive-columns-dark");
    app.widget().set_default_size(720, 740);
    wait_until(|| app.widget().width() < 1000);
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "archive-columns-narrow-light");

    let restored_location = VPath::from(bundle.join("Documents/nested.zip/Notes"));
    app.emit(AppMsg::NavigateExact(
        PaneId::Left,
        restored_location.clone(),
    ));
    wait_until(|| {
        visible(&app, PaneId::Left) == restored_location && !app.model().pane(PaneId::Left).loading
    });
    let saved = app
        .model()
        .archive_mounts
        .session(app.model().pane(PaneId::Left).to_session());
    assert!(
        saved
            .tabs
            .iter()
            .all(|path| !path.contains("commander-archive-"))
    );
    app.emit(AppMsg::NavigateExact(
        PaneId::Left,
        VPath::from(fixture.path()),
    ));
    wait_until(|| {
        visible(&app, PaneId::Left) == VPath::from(fixture.path())
            && !app.model().pane(PaneId::Left).loading
    });
    // A cancelled completion must not hijack the current location.
    let stale = open_location(
        &LocalFs,
        &VPath::from(bundle.as_path()),
        &[],
        &CancelToken::new(),
    )
    .unwrap();
    app.emit(AppMsg::ArchiveBrowseReady {
        pane: PaneId::Left,
        id: JobId::next(),
        result: Ok(stale),
    });
    app.emit(AppMsg::SetViewMode(PaneViewMode::List));
    wait_until(|| app.model().pane(PaneId::Left).view_mode == PaneViewMode::List);
    assert_eq!(visible(&app, PaneId::Left), VPath::from(fixture.path()));
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((VPath::from(bundle.as_path()), EntryKind::File)),
    ));
    app.emit(AppMsg::ExecuteCommand(CommandId::OpenInNewTab));
    wait_until(|| {
        app.model().pane(PaneId::Left).active_tab == 1
            && visible(&app, PaneId::Left) == VPath::from(bundle.as_path())
            && !app.model().pane(PaneId::Left).loading
    });
    app.emit(AppMsg::Up(PaneId::Left));
    wait_until(|| visible(&app, PaneId::Left) == VPath::from(fixture.path()));
    app.emit(AppMsg::SelectTab(PaneId::Left, 0));
    wait_until(|| {
        app.model().pane(PaneId::Left).active_tab == 0 && !app.model().pane(PaneId::Left).loading
    });
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((VPath::from(bundle.as_path()), EntryKind::File)),
    ));
    app.emit(AppMsg::ExecuteCommand(CommandId::BrowseArchive));
    app.emit(AppMsg::ExecuteCommand(CommandId::OpenOtherPane));
    wait_until(|| {
        visible(&app, PaneId::Left) == VPath::from(bundle.as_path())
            && visible(&app, PaneId::Right) == VPath::from(bundle.as_path())
    });
    assert_eq!(
        app.model().active_pane,
        PaneId::Right,
        "archive completions must not steal pane focus"
    );
    assert_eq!(std::fs::read(&bundle).unwrap(), original);
    app.widget().close();
    drop(app);

    let restored = launch(
        VPath::from(saved.tabs[saved.active_tab].as_str()),
        VPath::from(destination.as_path()),
        Some(SessionState {
            left: saved,
            workflow: WorkflowPreferences {
                browse_archives: false,
                ..WorkflowPreferences::default()
            },
            preview_visible: false,
            sidebar_visible: false,
            ..SessionState::default()
        }),
    );
    restored.widget().present();
    wait_until(|| {
        visible(&restored, PaneId::Left) == restored_location
            && !restored.model().pane(PaneId::Left).loading
            && restored
                .model()
                .pane(PaneId::Left)
                .archive_browse
                .source
                .is_none()
    });
    assert!(restored.model().pane(PaneId::Left).error.is_none());
    assert!(
        restored
            .model()
            .is_archive_browse_path(restored.model().pane(PaneId::Left).current_directory())
    );
    restored.emit(AppMsg::NavigateExact(
        PaneId::Left,
        VPath::from(fixture.path()),
    ));
    wait_until(|| {
        visible(&restored, PaneId::Left) == VPath::from(fixture.path())
            && !restored.model().pane(PaneId::Left).loading
    });
    restored.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((VPath::from(bundle.as_path()), EntryKind::File)),
    ));
    restored.emit(AppMsg::ExecuteCommand(CommandId::BrowseArchive));
    wait_until(|| visible(&restored, PaneId::Left) == VPath::from(bundle.as_path()));
    restored.widget().close();
}

fn descendants(root: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut nodes = vec![root.as_ref().clone()];
    let mut child = root.as_ref().first_child();
    while let Some(widget) = child {
        nodes.extend(descendants(&widget));
        child = widget.next_sibling();
    }
    nodes
}

fn dialog_button(dialog: &adw::Dialog, label: &str) -> gtk::Button {
    descendants(dialog)
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.label().as_deref() == Some(label))
        .unwrap()
}

fn submit_name(app: &relm4::Controller<AppModel>, command: CommandId, name: &str, button: &str) {
    app.emit(AppMsg::ExecuteCommand(command));
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    let nodes = descendants(&dialog);
    assert!(nodes.iter().any(
        |widget| widget.has_css_class("window-close") && widget.has_css_class("window-action")
    ));
    nodes
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Entry>().ok())
        .unwrap()
        .set_text(name);
    dialog_button(&dialog, button).emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_none());
}

fn root(app: &relm4::Controller<AppModel>, pane: PaneId) -> VPath {
    app.model().pane(pane).current_directory().clone()
}

fn updated(app: &relm4::Controller<AppModel>, previous: &VPath) {
    wait_until(|| {
        let model = app.model();
        model.active_operations == 0
            && model.panes.iter().all(|pane| !pane.loading)
            && model.pane(PaneId::Left).current_directory() != previous
            && model.pane(PaneId::Left).current_directory()
                == model.pane(PaneId::Right).current_directory()
    });
    assert!(
        app.model()
            .operations
            .values()
            .all(|operation| operation.error.is_none())
    );
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_archive_panel_updates_copy_paste_drop_rename_remove_and_refresh_tabs() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let original_file = fixture.path().join("original.txt");
    std::fs::write(&original_file, "original contents").unwrap();
    let bundle = fixture.path().join("Working archive.zip");
    let nested = fixture.path().join("nested.zip");
    archive(&original_file, &nested, ArchiveFormat::Zip);
    create_archive(
        &LocalFs,
        &[
            VPath::from(original_file.as_path()),
            VPath::from(nested.as_path()),
        ],
        &VPath::from(bundle.as_path()),
        ArchiveFormat::Zip,
        &mut ArchiveTask::new(&CancelToken::new()),
    )
    .unwrap();
    let original = std::fs::read(&bundle).unwrap();
    let app = launch(
        VPath::from(bundle.as_path()),
        VPath::from(bundle.as_path()),
        Some(SessionState {
            sidebar_visible: false,
            preview_visible: false,
            dual_pane: true,
            window_width: 1320,
            window_height: 800,
            ..SessionState::default()
        }),
    );
    app.widget().present();
    wait_until(|| {
        app.model()
            .panes
            .iter()
            .all(|pane| !pane.loading && pane.archive_browse.source.is_none())
            && visible(&app, PaneId::Left) == VPath::from(bundle.as_path())
            && root(&app, PaneId::Left) == root(&app, PaneId::Right)
    });
    app.emit(AppMsg::NewTab(PaneId::Left));
    wait_until(|| {
        app.model().pane(PaneId::Left).tabs.len() == 2 && !app.model().pane(PaneId::Left).loading
    });
    app.emit(AppMsg::ContextTarget(PaneId::Left, None));
    for (view, name) in [
        (PaneViewMode::List, "List folder"),
        (PaneViewMode::Grid, "Grid folder"),
        (PaneViewMode::Columns, "Columns folder"),
    ] {
        app.emit(AppMsg::SetViewMode(view));
        wait_until(|| {
            app.model().pane(PaneId::Left).view_mode == view
                && !app.model().pane(PaneId::Left).loading
        });
        let previous = root(&app, PaneId::Left);
        submit_name(&app, CommandId::NewDirectory, name, "Create");
        updated(&app, &previous);
        assert!(root(&app, PaneId::Left).as_path().join(name).is_dir());
        assert!(
            app.model()
                .pane(PaneId::Left)
                .tabs
                .iter()
                .all(|tab| tab.path == root(&app, PaneId::Left))
        );
    }
    app.emit(AppMsg::SetViewMode(PaneViewMode::List));
    wait_until(|| app.model().pane(PaneId::Left).view_mode == PaneViewMode::List);
    let previous = root(&app, PaneId::Left);
    submit_name(&app, CommandId::NewFile, "empty.txt", "Create");
    updated(&app, &previous);
    assert_eq!(
        std::fs::metadata(root(&app, PaneId::Left).as_path().join("empty.txt"))
            .unwrap()
            .len(),
        0
    );

    let incoming = fixture.path().join("Pasted folder");
    std::fs::create_dir_all(incoming.join("Empty")).unwrap();
    std::fs::write(incoming.join("note.txt"), "pasted contents").unwrap();
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::ClipboardFiles {
        pane: PaneId::Left,
        destination: previous.clone(),
        result: Ok((vec![VPath::from(incoming.as_path())], false)),
        owner: None,
    });
    updated(&app, &previous);
    assert!(
        root(&app, PaneId::Left)
            .as_path()
            .join("Pasted folder/Empty")
            .is_dir()
    );

    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((
            previous.join_name(OsStr::new("Pasted folder")),
            EntryKind::Directory,
        )),
    ));
    submit_name(&app, CommandId::Rename, "Renamed folder", "Rename");
    updated(&app, &previous);
    assert_eq!(
        std::fs::read_to_string(
            root(&app, PaneId::Left)
                .as_path()
                .join("Renamed folder/note.txt")
        )
        .unwrap(),
        "pasted contents"
    );

    // Dragging a duplicate uses the app's existing conflict sheet.
    std::fs::write(&original_file, "replacement contents").unwrap();
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::DropFiles {
        sources: vec![VPath::from(original_file.as_path())],
        destination: previous.clone(),
        action: FileDropAction::Copy,
    });
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    assert!(
        descendants(&dialog)
            .iter()
            .any(|widget| widget.has_css_class("window-close"))
    );
    snapshot(app.widget(), "archive-panel-replace-dark");
    dialog_button(&dialog, "Replace").emit_clicked();
    updated(&app, &previous);
    wait_until(|| app.widget().visible_dialog().is_none());
    assert_eq!(
        std::fs::read_to_string(root(&app, PaneId::Left).as_path().join("original.txt")).unwrap(),
        "replacement contents"
    );

    // Confirmation captures its target, even if selection changes while it is open.
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((
            previous.join_name(OsStr::new("Renamed folder")),
            EntryKind::Directory,
        )),
    ));
    app.emit(AppMsg::ExecuteCommand(CommandId::Trash));
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "archive-panel-remove-light");
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((
            previous.join_name(OsStr::new("original.txt")),
            EntryKind::File,
        )),
    ));
    wait_until(|| {
        app.model()
            .focused_item(PaneId::Left)
            .is_some_and(|(path, _)| path.file_name().is_some_and(|name| name == "original.txt"))
    });
    dialog_button(&dialog, "Remove").emit_clicked();
    updated(&app, &previous);
    assert!(
        !root(&app, PaneId::Left)
            .as_path()
            .join("Renamed folder")
            .exists()
    );
    assert!(
        root(&app, PaneId::Left)
            .as_path()
            .join("original.txt")
            .exists()
    );
    assert!(
        std::fs::read_dir(fixture.path())
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".commander-archive-backup-")
                && std::fs::read(entry.path()).unwrap() == original)
    );
    wait_until(|| app.widget().visible_dialog().is_none());
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::NavigateExact(
        PaneId::Right,
        VPath::from(bundle.join("nested.zip")),
    ));
    wait_until(|| {
        visible(&app, PaneId::Right) == VPath::from(bundle.join("nested.zip"))
            && !app.model().pane(PaneId::Right).loading
            && app
                .model()
                .pane(PaneId::Right)
                .archive_browse
                .source
                .is_none()
    });
    assert!(app.widgets().panes[1].status.text().contains("read-only"));
    let old_nested = root(&app, PaneId::Right);
    app.emit(AppMsg::ContextTarget(PaneId::Left, None));
    submit_name(&app, CommandId::NewDirectory, "After nested view", "Create");
    updated(&app, &previous);
    assert_eq!(visible(&app, PaneId::Right), VPath::from(bundle.as_path()));
    assert!(
        old_nested.as_path().join("original.txt").exists(),
        "Retired snapshots must stay available to copies already in progress"
    );
    app.emit(AppMsg::NavigateExact(
        PaneId::Right,
        VPath::from(bundle.join("nested.zip")),
    ));
    wait_until(|| {
        visible(&app, PaneId::Right) == VPath::from(bundle.join("nested.zip"))
            && !app.model().pane(PaneId::Right).loading
            && app
                .model()
                .pane(PaneId::Right)
                .archive_browse
                .source
                .is_none()
    });
    assert_ne!(
        root(&app, PaneId::Right),
        old_nested,
        "Reopening must use the updated outer archive"
    );

    // Finishing after navigation refreshes inactive tabs without redirecting the user.
    let previous = root(&app, PaneId::Left);
    let later = fixture.path().join("late.txt");
    std::fs::write(&later, "late import").unwrap();
    app.emit(AppMsg::ClipboardFiles {
        pane: PaneId::Left,
        destination: previous.clone(),
        result: Ok((vec![VPath::from(later.as_path())], false)),
        owner: None,
    });
    for pane in [PaneId::Left, PaneId::Right] {
        app.emit(AppMsg::NavigateExact(pane, VPath::from(fixture.path())));
    }
    wait_until(|| {
        app.model().active_operations == 0
            && app.model().pane(PaneId::Left).tabs[0].path != previous
    });
    assert_eq!(root(&app, PaneId::Left), VPath::from(fixture.path()));
    assert_eq!(root(&app, PaneId::Right), VPath::from(fixture.path()));
    app.emit(AppMsg::SelectTab(PaneId::Left, 0));
    wait_until(|| {
        visible(&app, PaneId::Left) == VPath::from(bundle.as_path())
            && !app.model().pane(PaneId::Left).loading
    });
    assert_eq!(
        std::fs::read_to_string(root(&app, PaneId::Left).as_path().join("late.txt")).unwrap(),
        "late import"
    );
    app.widget().close();
}

fn archive_changed(app: &relm4::Controller<AppModel>, previous: &VPath) {
    wait_until(|| {
        let model = app.model();
        !model.history_busy
            && model.active_operations == 0
            && !model.pane(PaneId::Left).loading
            && model.pane(PaneId::Left).archive_browse.source.is_none()
            && model.pane(PaneId::Left).current_directory() != previous
    });
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_archive_actions_undo_redo_restart_and_recovery() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let file = fixture.path().join("notes.txt");
    std::fs::write(&file, "original notes").unwrap();
    let bundle = fixture.path().join("Undo and recovery.zip");
    archive(&file, &bundle, ArchiveFormat::Zip);
    let original = std::fs::read(&bundle).unwrap();
    let stale = open_location(
        &LocalFs,
        &VPath::from(bundle.as_path()),
        &[],
        &CancelToken::new(),
    )
    .unwrap();
    let session = SessionState {
        sidebar_visible: false,
        preview_visible: false,
        dual_pane: true,
        window_width: 1320,
        window_height: 800,
        ..SessionState::default()
    };
    let app = launch(
        bundle.clone().into(),
        fixture.path().into(),
        Some(session.clone()),
    );
    app.widget().present();
    wait_until(|| {
        app.model()
            .pane(PaneId::Left)
            .archive_browse
            .source
            .is_none()
            && !app.model().pane(PaneId::Left).loading
            && visible(&app, PaneId::Left) == VPath::from(bundle.as_path())
    });
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((
            root(&app, PaneId::Left).join_name(OsStr::new("notes.txt")),
            EntryKind::File,
        )),
    ));
    wait_until(|| app.model().action_context(PaneId::Left).items == 1);
    let policy = app.model().action_context(PaneId::Left);
    assert_eq!(
        policy.action(CommandId::Trash).label("Trash"),
        "Remove from archive…"
    );
    assert!(!policy.action(CommandId::EditFile).enabled());
    app.emit(AppMsg::ExecuteCommand(CommandId::CommandPalette));
    wait_until(|| app.model().palette_open);
    let items = app.model().palette_items();
    assert!(items.iter().any(|item| matches!(
        item.action,
        PaletteAction::Command(CommandId::Trash)
    ) && item.label == "Remove from archive…"));
    assert!(!items.iter().any(|item| matches!(
        item.action,
        PaletteAction::Command(CommandId::EditFile | CommandId::OpenWith | CommandId::Move)
    )));
    app.emit(AppMsg::ClosePalette);
    wait_until(|| !app.model().palette_open && app.widget().visible_dialog().is_none());
    let previous = root(&app, PaneId::Left);
    submit_name(&app, CommandId::NewDirectory, "Saved change", "Create");
    archive_changed(&app, &previous);
    let updated = std::fs::read(&bundle).unwrap();
    assert_ne!(updated, original);
    assert!(matches!(
        app.model().undo_stack.last(),
        Some(HistoryEntry::Archive { .. })
    ));
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::ExecuteCommand(CommandId::Undo));
    archive_changed(&app, &previous);
    assert_eq!(std::fs::read(&bundle).unwrap(), original);
    assert!(
        !root(&app, PaneId::Left)
            .as_path()
            .join("Saved change")
            .exists()
    );
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::ExecuteCommand(CommandId::Redo));
    archive_changed(&app, &previous);
    assert_eq!(std::fs::read(&bundle).unwrap(), updated);
    wait_until(|| {
        let (saved, warning) = crate::history_store::load(&crate::history_store::path().unwrap());
        warning.is_none()
            && matches!(saved.undo.last(), Some(HistoryEntry::Archive { .. }))
            && saved.pending.is_none()
    });
    app.widget().close();
    drop(app);

    let app = launch(bundle.clone().into(), fixture.path().into(), Some(session));
    app.widget().present();
    wait_until(|| {
        app.model()
            .pane(PaneId::Left)
            .archive_browse
            .source
            .is_none()
            && !app.model().pane(PaneId::Left).loading
            && visible(&app, PaneId::Left) == VPath::from(bundle.as_path())
    });
    assert!(
        app.model()
            .action_context(PaneId::Left)
            .action(CommandId::Undo)
            .enabled()
    );
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::ExecuteCommand(CommandId::Undo));
    archive_changed(&app, &previous);
    assert_eq!(std::fs::read(&bundle).unwrap(), original);
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::ExecuteCommand(CommandId::Redo));
    archive_changed(&app, &previous);
    assert_eq!(std::fs::read(&bundle).unwrap(), updated);

    app.emit(AppMsg::ShowRecovery);
    wait_until(|| {
        app.widget().visible_dialog().is_some()
            && app
                .model()
                .recovery_records
                .iter()
                .any(|record| record.archive.is_some())
    });
    let dialog = app.widget().visible_dialog().unwrap();
    let row = descendants(&dialog)
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| {
            button
                .label()
                .is_some_and(|label| label.starts_with("Update archive"))
        })
        .unwrap();
    row.emit_clicked();
    wait_until(|| {
        app.widget().visible_dialog().is_some_and(|dialog| {
            descendants(&dialog)
                .iter()
                .filter_map(|widget| widget.downcast_ref::<gtk::Button>())
                .any(|button| button.label().as_deref() == Some("Restore archive"))
        })
    });
    let detail = app.widget().visible_dialog().unwrap();
    assert!(descendants(&detail).iter().any(
        |widget| widget.has_css_class("window-close") && widget.has_css_class("window-action")
    ));
    snapshot(app.widget(), "archive-recovery-details-dark");
    let previous = root(&app, PaneId::Left);
    dialog_button(&detail, "Restore archive").emit_clicked();
    archive_changed(&app, &previous);
    assert_eq!(std::fs::read(&bundle).unwrap(), original);
    while let Some(dialog) = app.widget().visible_dialog() {
        dialog.close();
        wait_until(|| app.widget().visible_dialog().as_ref() != Some(&dialog));
    }
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::ExecuteCommand(CommandId::Undo));
    archive_changed(&app, &previous);
    assert_eq!(std::fs::read(&bundle).unwrap(), updated);
    let current = root(&app, PaneId::Left);
    app.emit(AppMsg::ArchiveReloadReady {
        source: bundle.clone().into(),
        id: JobId::next(),
        result: Ok(stale),
    });
    app.emit(AppMsg::SetPaletteQuery("stale refresh processed".into()));
    wait_until(|| app.model().palette_query == "stale refresh processed");
    assert_eq!(root(&app, PaneId::Left), current);
    assert!(current.as_path().join("Saved change").exists());
    app.widget().close();
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_archive_password_creation_unlock_retry_edit_extract_and_cancel() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let notes = fixture.path().join("notes.txt");
    std::fs::write(&notes, "protected notes").unwrap();
    let bundle = fixture.path().join("Protected.zip");
    let other = fixture.path().join("Another.7z");
    let secret = "synthetic-ui-password";
    let mut task = ArchiveTask::new(&CancelToken::new());
    task.set_password(crate::archive::Password::new(secret.into()));
    create_archive(
        &LocalFs,
        &[notes.clone().into()],
        &other.clone().into(),
        ArchiveFormat::SevenZ,
        &mut task,
    )
    .unwrap();
    let app = launch(
        fixture.path().into(),
        output.path().into(),
        Some(SessionState {
            sidebar_visible: false,
            preview_visible: false,
            dual_pane: true,
            window_width: 1280,
            window_height: 800,
            ..SessionState::default()
        }),
    );
    app.widget().present();
    wait_until(|| !app.model().pane(PaneId::Left).loading);
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((notes.into(), EntryKind::File)),
    ));
    app.emit(AppMsg::ExecuteCommand(CommandId::CreateArchive));
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    assert_eq!(dialog.title(), "Create Archive");
    let nodes = descendants(&dialog);
    assert!(nodes.iter().any(
        |widget| widget.has_css_class("window-close") && widget.has_css_class("window-action")
    ));
    nodes
        .iter()
        .find_map(|widget| widget.clone().downcast::<gtk::Entry>().ok())
        .unwrap()
        .set_text("Protected");
    let protect = nodes
        .iter()
        .find_map(|widget| widget.clone().downcast::<gtk::CheckButton>().ok())
        .unwrap();
    let format = nodes
        .iter()
        .find_map(|widget| widget.clone().downcast::<gtk::DropDown>().ok())
        .unwrap();
    let fields: Vec<_> = nodes
        .iter()
        .filter_map(|widget| widget.clone().downcast::<gtk::PasswordEntry>().ok())
        .collect();
    assert_eq!(fields.len(), 2);
    protect.set_active(true);
    assert!(!dialog_button(&dialog, "Create").is_sensitive());
    fields[0].set_text(secret);
    fields[1].set_text("mismatch");
    assert!(!dialog_button(&dialog, "Create").is_sensitive());
    fields[1].set_text(secret);
    assert!(dialog_button(&dialog, "Create").is_sensitive());
    format.set_selected(2);
    assert!(!protect.is_sensitive() && !protect.is_active());
    format.set_selected(0);
    protect.set_active(true);
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    snapshot(app.widget(), "archive-create-password-dark");
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight);
    snapshot(app.widget(), "archive-create-password-light");
    dialog_button(&dialog, "Create").emit_clicked();
    wait_until(|| {
        bundle.exists()
            && app.model().active_operations == 0
            && app.widget().visible_dialog().is_none()
    });
    assert!(fields.iter().all(|field| field.text().is_empty()));
    let original = std::fs::read(&bundle).unwrap();
    app.emit(AppMsg::Navigate(PaneId::Left, bundle.clone().into()));
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    assert_eq!(dialog.title(), "Unlock Archive");
    let field = descendants(&dialog)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::PasswordEntry>().ok())
        .unwrap();
    assert!(!dialog_button(&dialog, "Unlock").is_sensitive());
    field.set_text("wrong");
    dialog_button(&dialog, "Unlock").emit_clicked();
    wait_until(|| {
        app.widget()
            .visible_dialog()
            .is_some_and(|next| next != dialog)
    });
    assert!(field.text().is_empty());
    let dialog = app.widget().visible_dialog().unwrap();
    assert!(descendants(&dialog).iter().any(|node| {
        node.downcast_ref::<gtk::Label>()
            .is_some_and(|label| label.text().contains("not accepted"))
    }));
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    snapshot(app.widget(), "archive-unlock-retry-dark");
    let unlock = |dialog: &adw::Dialog| {
        descendants(dialog)
            .into_iter()
            .find_map(|widget| widget.downcast::<gtk::PasswordEntry>().ok())
            .unwrap()
            .set_text(secret);
        dialog_button(dialog, "Unlock").emit_clicked();
    };
    unlock(&dialog);
    wait_until(|| {
        visible(&app, PaneId::Left) == VPath::from(bundle.as_path())
            && app
                .model()
                .pane(PaneId::Left)
                .archive_browse
                .source
                .is_none()
            && !app.model().pane(PaneId::Left).loading
    });
    let previous = root(&app, PaneId::Left);
    submit_name(&app, CommandId::NewFile, "new.txt", "Create");
    archive_changed(&app, &previous);
    assert!(root(&app, PaneId::Left).as_path().join("new.txt").exists());
    assert!(
        app.widget().visible_dialog().is_none(),
        "edits should reuse the unlocked archive password"
    );
    let previous = root(&app, PaneId::Left);
    app.emit(AppMsg::ExecuteCommand(CommandId::Undo));
    archive_changed(&app, &previous);
    assert_eq!(std::fs::read(&bundle).unwrap(), original);
    app.emit(AppMsg::Navigate(PaneId::Left, fixture.path().into()));
    wait_until(|| {
        root(&app, PaneId::Left) == VPath::from(fixture.path())
            && !app.model().pane(PaneId::Left).loading
    });
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((bundle.clone().into(), EntryKind::File)),
    ));
    app.emit(AppMsg::ExecuteCommand(CommandId::ExtractArchive));
    wait_until(|| app.widget().visible_dialog().is_some());
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
    unlock(&app.widget().visible_dialog().unwrap());
    wait_until(|| app.model().active_operations == 0 && output.path().join("notes.txt").exists());
    assert_eq!(
        std::fs::read_to_string(output.path().join("notes.txt")).unwrap(),
        "protected notes"
    );
    app.emit(AppMsg::Navigate(PaneId::Left, other.clone().into()));
    wait_until(|| app.widget().visible_dialog().is_some());
    dialog_button(&app.widget().visible_dialog().unwrap(), "Cancel").emit_clicked();
    wait_until(|| {
        app.widget().visible_dialog().is_none()
            && app
                .model()
                .pane(PaneId::Left)
                .archive_browse
                .source
                .is_none()
    });
    assert_eq!(root(&app, PaneId::Left), VPath::from(fixture.path()));
    assert!(app.model().pane(PaneId::Left).error.is_none());
    app.emit(AppMsg::Navigate(PaneId::Left, other.into()));
    wait_until(|| app.widget().visible_dialog().is_some());
    app.emit(AppMsg::Navigate(PaneId::Left, output.path().into()));
    wait_until(|| {
        app.widget().visible_dialog().is_none()
            && root(&app, PaneId::Left) == VPath::from(output.path())
    });
    assert!(app.model().pane(PaneId::Left).error.is_none());
    app.widget().close();
}
