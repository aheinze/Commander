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
    let inner_open = open_location(&LocalFs, &inner_path, &[], &CancelToken::new()).unwrap();
    let paths = ArchiveLocations {
        mounts: vec![outer_open.mount, inner_open.mount],
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
    assert!(
        app.widgets().panes[0]
            .status
            .text()
            .contains("Archive · read-only")
    );
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
            && app.widgets().preview.read_only_value.text() == "Yes"
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
        "archive restrictions must override the writable staging permissions"
    );
    reveal_inspector_metadata(&app);
    snapshot(app.widget(), "archive-member-inspector-dark");
    app.emit(AppMsg::ExecuteCommand(CommandId::TogglePreview));
    app.emit(AppMsg::ExecuteCommand(CommandId::Trash));
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
