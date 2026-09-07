use super::*;
use relm4::{Component, ComponentController};

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let context = glib::MainContext::default();
    while !ready() {
        assert!(Instant::now() < deadline, "archive UI timed out");
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn descendant<T: IsA<gtk::Widget> + glib::object::IsClass>(
    root: &impl IsA<gtk::Widget>,
) -> Option<T> {
    let root = root.as_ref();
    if let Ok(widget) = root.clone().downcast::<T>() {
        return Some(widget);
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(found) = descendant::<T>(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

fn create_button(root: &impl IsA<gtk::Widget>) -> Option<gtk::Button> {
    let root = root.as_ref();
    if let Some(button) = root.downcast_ref::<gtk::Button>()
        && button.label().as_deref() == Some("Create")
    {
        return Some(button.clone());
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(found) = create_button(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

#[test]
#[ignore = "requires an isolated GTK display/session; run alone with --ignored --test-threads=1"]
fn gtk_archive_selected_folder_beside_it_and_round_trip_all_contents() {
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("Large folder");
    std::fs::create_dir_all(source.join("00-empty")).unwrap();
    let mut expected = Vec::new();
    for index in 0..48 {
        let name = format!("payload-{index:02}.bin");
        let bytes = vec![index as u8; 128 * 1024 + index];
        std::fs::write(source.join(&name), &bytes).unwrap();
        expected.push((name, bytes));
    }
    std::fs::create_dir_all(source.join("Nested/deep")).unwrap();
    std::fs::write(source.join("Nested/deep/note.txt"), b"nested payload").unwrap();
    expected.push((
        "Nested/deep/note.txt".to_owned(),
        b"nested payload".to_vec(),
    ));
    std::fs::write(source.join(".hidden"), b"hidden payload").unwrap();
    expected.push((".hidden".to_owned(), b"hidden payload".to_vec()));
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let mut session = SessionState {
        dual_pane: false,
        sidebar_visible: false,
        preview_visible: false,
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
    wait_until(|| {
        !app.model().pane(PaneId::Left).loading && !app.widgets().panes[0].miller_columns.is_empty()
    });
    let root_view = app.widgets().panes[0].miller_columns[0]
        .model
        .clone()
        .unwrap();
    root_view.select_item(0, true);
    app.emit(AppMsg::MillerOpen(PaneId::Left, 0, 0));
    wait_until(|| {
        app.model().pane(PaneId::Left).miller_columns.len() == 2
            && !app.model().pane(PaneId::Left).loading
    });
    assert_eq!(
        app.model().operation_sources(PaneId::Left),
        vec![VPath::from(source.as_path())],
        "opening a folder must keep it selected for archiving, instead of its first child"
    );
    assert_eq!(
        app.model().focused_path(PaneId::Left),
        Some(VPath::from(source.as_path()))
    );

    // Live updates in the child column must not replace the selected parent.
    let marker = source.join("watch-marker");
    std::fs::write(&marker, b"watch update").unwrap();
    let marker_visible = || {
        app.model().pane(PaneId::Left).miller_columns[1]
            .listing
            .as_ref()
            .unwrap()
            .rows()
            .any(|entry| entry.name() == OsStr::new("watch-marker"))
    };
    wait_until(marker_visible);
    assert_eq!(
        app.model().operation_sources(PaneId::Left),
        vec![VPath::from(source.as_path())]
    );
    std::fs::remove_file(marker).unwrap();
    wait_until(|| !marker_visible());

    // Keyboard movement and marking explicitly enter the child column.
    app.emit(AppMsg::MoveCursorTo(CursorTarget::Last, false));
    wait_until(|| {
        app.model().operation_sources(PaneId::Left)
            == vec![VPath::from(source.join("payload-47.bin"))]
    });
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((VPath::from(source.as_path()), EntryKind::Directory)),
    ));
    wait_until(|| {
        app.model().operation_sources(PaneId::Left) == vec![VPath::from(source.as_path())]
    });
    app.emit(AppMsg::ToggleCursor);
    wait_until(|| {
        app.model().operation_sources(PaneId::Left)
            == vec![VPath::from(source.join("payload-47.bin"))]
    });
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((VPath::from(source.as_path()), EntryKind::Directory)),
    ));
    wait_until(|| {
        app.model().operation_sources(PaneId::Left) == vec![VPath::from(source.as_path())]
    });

    app.emit(AppMsg::InvertSelectionActive);
    wait_until(|| app.model().pane(PaneId::Left).selection.len() == 50);
    assert!(
        app.model()
            .operation_sources(PaneId::Left)
            .iter()
            .all(|path| path.parent() == Some(VPath::from(source.as_path())))
    );
    app.emit(AppMsg::ContextTarget(
        PaneId::Left,
        Some((VPath::from(source.as_path()), EntryKind::Directory)),
    ));
    wait_until(|| {
        app.model().operation_sources(PaneId::Left) == vec![VPath::from(source.as_path())]
    });

    for (format, dropdown_index) in [
        (ArchiveFormat::Zip, 0),
        (ArchiveFormat::SevenZ, 1),
        (ArchiveFormat::Tar, 2),
        (ArchiveFormat::TarGz, 3),
    ] {
        // Right-clicking the already-open folder must also keep the branch and
        // target that folder, even after choosing a child in its contents.
        if format != ArchiveFormat::Zip {
            app.emit(AppMsg::ContextTarget(
                PaneId::Left,
                Some((VPath::from(source.join("payload-00.bin")), EntryKind::File)),
            ));
            app.emit(AppMsg::ContextTarget(
                PaneId::Left,
                Some((VPath::from(source.as_path()), EntryKind::Directory)),
            ));
            wait_until(|| {
                app.model().operation_sources(PaneId::Left) == vec![VPath::from(source.as_path())]
            });
            assert_eq!(app.model().pane(PaneId::Left).miller_columns.len(), 2);
        }
        let before = app.model().operations.len();
        app.emit(AppMsg::ExecuteCommand(CommandId::CreateArchive));
        wait_until(|| app.widget().visible_dialog().is_some());
        let dialog = app.widget().visible_dialog().unwrap();
        let entry = descendant::<gtk::Entry>(&dialog).unwrap();
        let dropdown = descendant::<gtk::DropDown>(&dialog).unwrap();
        let name = format!("backup-{}", format.extension());
        entry.set_text(&name);
        dropdown.set_selected(dropdown_index);
        create_button(&dialog).unwrap().emit_clicked();
        wait_until(|| app.model().operations.len() > before && app.model().active_operations == 0);
        let model = app.model();
        let job = model.operations.values().last().unwrap();
        assert_eq!(job.state, JobState::Done, "{format:?}: {:?}", job.error);
        assert_eq!(job.progress.items_done, expected.len() as u64 + 4);
        drop(model);
        let filename = format!("{name}.{}", format.extension());
        let archive = fixture.path().join(&filename);
        assert!(
            archive.is_file(),
            "archive belongs beside the selected folder: {}",
            archive.display()
        );
        assert!(!source.join(&filename).exists());
        let output = tempfile::tempdir().unwrap();
        extract_archive(
            &LocalFs,
            &VPath::from(archive.as_path()),
            &VPath::from(output.path()),
            &mut ArchiveTask::new(&CancelToken::new()),
        )
        .unwrap();
        assert!(output.path().join("Large folder/00-empty").is_dir());
        for (name, bytes) in &expected {
            assert_eq!(
                &std::fs::read(output.path().join("Large folder").join(name)).unwrap(),
                bytes,
                "{format:?}: {name}"
            );
        }
    }
    app.widget().destroy();
}
