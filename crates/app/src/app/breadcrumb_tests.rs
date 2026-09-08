use super::*;
use relm4::{Component, ComponentController};

#[test]
#[ignore = "requires an isolated GTK session; run alone with --ignored --test-threads=1"]
fn gtk_breadcrumbs_navigate_exactly_and_preserve_location_edits() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("Start");
    let branch = root.join("Branch");
    let leaf = branch.join("Leaf");
    std::fs::create_dir_all(&leaf).unwrap();
    std::fs::write(leaf.join("note.txt"), b"breadcrumb fixture").unwrap();
    let root = VPath::from(root);
    let leaf = VPath::from(leaf);
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(root.clone()),
                right: Some(root.clone()),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                sidebar_visible: false,
                preview_visible: false,
                dual_pane: false,
                vertical_split: false,
                window_width: 900,
                window_height: 720,
                ..SessionState::default()
            }),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    wait_until(|| !app.model().pane(PaneId::Left).loading);
    let mut problems = Vec::new();
    for mode in [
        PaneViewMode::List,
        PaneViewMode::Grid,
        PaneViewMode::Columns,
    ] {
        app.emit(AppMsg::Navigate(PaneId::Left, root.clone()));
        app.emit(AppMsg::SetViewMode(mode));
        wait_until(|| ready(&app, &root) && app.model().pane(PaneId::Left).view_mode == mode);
        if mode == PaneViewMode::Columns {
            for (column, name) in [(0, "Branch"), (1, "Leaf")] {
                let row = app.model().pane(PaneId::Left).miller_columns[column]
                    .listing
                    .as_ref()
                    .unwrap()
                    .rows()
                    .position(|entry| entry.name() == name)
                    .unwrap() as u32;
                app.emit(AppMsg::MillerOpen(PaneId::Left, column, row));
                wait_until(|| {
                    let model = app.model();
                    model
                        .pane(PaneId::Left)
                        .miller_columns
                        .get(column + 1)
                        .is_some_and(|column| column.listing.is_some() && !column.loading)
                });
            }
        } else {
            app.emit(AppMsg::Navigate(PaneId::Left, leaf.clone()));
            wait_until(|| ready(&app, &leaf));
        }
        app.emit(AppMsg::SelectAllActive);
        wait_until(|| app.model().pane(PaneId::Left).selection.len() == 1);
        let (generation, history) = {
            let model = app.model();
            let pane = model.pane(PaneId::Left);
            (pane.generation, pane.active().history.len())
        };
        click_crumb(&app, &leaf);
        drain();
        {
            let model = app.model();
            let pane = model.pane(PaneId::Left);
            if pane.generation != generation
                || pane.active().history.len() != history
                || pane.selection.len() != 1
            {
                problems.push(format!(
                    "{mode:?}: current-folder click reloaded files or changed history/selection"
                ));
            }
        }
        click_crumb(&app, &root);
        drain();
        wait_until(|| !app.model().pane(PaneId::Left).loading);
        if !ready(&app, &root) {
            problems.push(format!("{mode:?}: ancestor click restored a deeper branch"));
        } else {
            app.emit(AppMsg::Back(PaneId::Left));
            wait_until(|| ready(&app, &leaf));
            app.emit(AppMsg::Forward(PaneId::Left));
            wait_until(|| ready(&app, &root));
        }
        app.emit(AppMsg::SetViewMode(PaneViewMode::List));
        app.emit(AppMsg::Navigate(PaneId::Left, leaf.clone()));
        wait_until(|| ready(&app, &leaf));
    }

    app.emit(AppMsg::ExecuteCommand(CommandId::FocusLocation));
    wait_until(|| {
        app.widgets().panes[0]
            .breadcrumb_stack
            .visible_child_name()
            .as_deref()
            == Some("location")
    });
    let entry = app.widgets().panes[0].path_entry.clone();
    let draft = format!("{root}/unfinished edit");
    entry.set_text(&draft);
    app.emit(AppMsg::ActivatePane(PaneId::Left));
    drain();
    if entry.text() != draft {
        problems.push("A view update overwrote an unfinished location edit".to_owned());
    }
    app.widgets().panes[0].column_view.grab_focus();
    drain();
    if app.widgets().panes[0]
        .breadcrumb_stack
        .visible_child_name()
        .as_deref()
        != Some("breadcrumbs")
    {
        problems.push("Leaving the location field did not restore breadcrumbs".to_owned());
    }

    app.emit(AppMsg::SelectAllActive);
    app.emit(AppMsg::ExecuteCommand(CommandId::FocusLocation));
    wait_until(|| {
        app.widgets().panes[0]
            .breadcrumb_stack
            .visible_child_name()
            .as_deref()
            == Some("location")
    });
    assert_eq!(
        entry.text(),
        leaf.to_string(),
        "Reopening location must discard a canceled draft"
    );
    entry.set_text(&draft);
    let mut handled = false;
    for widget in [app.widget().upcast_ref::<gtk::Widget>(), entry.upcast_ref()] {
        let controllers = widget.observe_controllers();
        for index in 0..controllers.n_items() {
            if let Some(controller) = controllers
                .item(index)
                .and_downcast::<gtk::EventControllerKey>()
                && controller.emit_by_name::<bool>(
                    "key-pressed",
                    &[&gdk::Key::Escape, &0_u32, &gdk::ModifierType::empty()],
                )
            {
                assert_eq!(
                    widget,
                    entry.upcast_ref::<gtk::Widget>(),
                    "The location field must own Escape"
                );
                handled = true;
                break;
            }
        }
        if handled {
            break;
        }
    }
    assert!(handled);
    wait_until(|| {
        app.widgets().panes[0]
            .breadcrumb_stack
            .visible_child_name()
            .as_deref()
            == Some("breadcrumbs")
    });
    assert_eq!(app.model().pane(PaneId::Left).selection.len(), 1);

    let deep = fixture.path().join("One/Two/Three/Four/Five/Six");
    std::fs::create_dir_all(&deep).unwrap();
    let deep = VPath::from(deep);
    app.emit(AppMsg::Navigate(PaneId::Left, deep.clone()));
    wait_until(|| ready(&app, &deep));
    let overflow =
        descendants::<gtk::MenuButton>(app.widgets().panes[0].breadcrumb_stack.upcast_ref());
    if overflow.is_empty() {
        problems.push("Hidden ancestors have no clickable overflow menu".to_owned());
    } else {
        overflow[0].popup();
        let popover = overflow[0].popover().unwrap();
        wait_until(|| popover.is_mapped());
        let list = descendants::<gtk::ListBox>(popover.upcast_ref()).remove(0);
        list.row_at_index(0).unwrap().grab_focus();
        assert_window_defers(&app, gdk::Key::Down);
        drain();
        if let Some(directory) = std::env::var_os("COMMANDER_BREADCRUMB_SNAPSHOT_DIR") {
            let child = popover.first_child().unwrap();
            let snapshot = gtk::Snapshot::new();
            popover.snapshot_child(&child, &snapshot);
            popover
                .renderer()
                .unwrap()
                .render_texture(snapshot.to_node().unwrap(), None)
                .save_to_png(std::path::Path::new(&directory).join("breadcrumb-parents.png"))
                .unwrap();
        }
        let row = descendants::<gtk::ListBoxRow>(list.upcast_ref())
            .into_iter()
            .find(|row| row.tooltip_text().as_deref() == fixture.path().to_str())
            .unwrap();
        list.emit_by_name::<()>("row-activated", &[&row]);
        wait_until(|| ready(&app, &VPath::from(fixture.path())));
    }

    // The fixed parent menu and the current crumb must remain accessible in narrow panes.
    let long = fixture
        .path()
        .join("A long descriptive folder name ".repeat(4))
        .join("Another long folder name ".repeat(4));
    std::fs::create_dir_all(&long).unwrap();
    let long = VPath::from(long);
    app.emit(AppMsg::Navigate(PaneId::Left, long.clone()));
    app.emit(AppMsg::ToggleDualPane);
    app.widget().set_default_size(720, 540);
    wait_until(|| ready(&app, &long) && app.widget().width() == 720 && app.model().dual_pane);
    drain();
    let widgets = app.widgets();
    let pane = &widgets.panes[0];
    let current = descendants::<gtk::Button>(pane.breadcrumb_box.upcast_ref())
        .pop()
        .unwrap();
    let current_bounds = current.compute_bounds(&pane.breadcrumb_stack).unwrap();
    let overflow_bounds = pane
        .breadcrumb_overflow
        .compute_bounds(&pane.breadcrumb_stack)
        .unwrap();
    assert!(
        current_bounds.x() >= 0.0
            && current_bounds.x() + current_bounds.width()
                <= pane.breadcrumb_stack.width() as f32 + 1.0
    );
    assert!(overflow_bounds.x() >= 0.0 && pane.breadcrumb_overflow.is_mapped());
    drop(widgets);
    snapshot(&app, "breadcrumbs-narrow");
    app.widget().set_default_size(900, 540);
    wait_until(|| app.widget().width() == 900 && app.widgets().panes[0].root.width() < 500);
    drain();
    let widgets = app.widgets();
    let pane = &widgets.panes[0];
    let current = descendants::<gtk::Button>(pane.breadcrumb_box.upcast_ref())
        .pop()
        .unwrap();
    for control in [
        current.upcast_ref::<gtk::Widget>(),
        pane.breadcrumb_overflow.upcast_ref(),
    ] {
        let bounds = control.compute_bounds(&pane.root).unwrap();
        assert!(bounds.x() >= 0.0 && bounds.x() + bounds.width() <= pane.root.width() as f32 + 1.0);
    }
    drop(widgets);
    snapshot(&app, "breadcrumbs-side-by-side");
    // A breadcrumb on the inactive pane must activate and navigate that pane alone.
    let right = app.widgets().panes[1].breadcrumb_box.clone();
    let right_parent = descendants::<gtk::Button>(right.upcast_ref())
        .into_iter()
        .find(|button| button.tooltip_text().as_deref() == fixture.path().to_str())
        .unwrap();
    right_parent.emit_clicked();
    wait_until(|| {
        app.model().active_pane == PaneId::Right && !app.model().pane(PaneId::Right).loading
    });
    assert!(ready(&app, &long));
    assert_eq!(
        app.model().pane(PaneId::Right).current_directory(),
        &VPath::from(fixture.path())
    );

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let paths = [0xff, 0xfe].map(|byte| {
            let path = fixture.path().join(OsString::from_vec(vec![b'N', byte]));
            std::fs::create_dir(&path).unwrap();
            VPath::from(path)
        });
        for path in &paths {
            app.emit(AppMsg::Navigate(PaneId::Left, path.clone()));
            wait_until(|| ready(&app, path));
        }
        let buttons =
            descendants::<gtk::Button>(app.widgets().panes[0].breadcrumb_box.upcast_ref());
        let current = buttons.last().unwrap();
        if descendants::<gtk::Label>(current.upcast_ref())
            .iter()
            .any(|label| label.text() == "/")
        {
            problems.push("A non-UTF-8 folder is incorrectly labeled as root".to_owned());
        }
        current.emit_clicked();
        drain();
        if !ready(&app, &paths[1]) {
            problems.push(
                "Non-UTF-8 folders with the same display text retained a stale click target"
                    .to_owned(),
            );
        }
    }
    app.widget().close();
    assert!(
        problems.is_empty(),
        "Breadcrumb failures:\n{}",
        problems.join("\n")
    );
}

fn ready(app: &relm4::Controller<AppModel>, path: &VPath) -> bool {
    let model = app.model();
    let state = model.pane(PaneId::Left);
    !state.loading && state.current_directory() == path
}

fn click_crumb(app: &relm4::Controller<AppModel>, path: &VPath) {
    let buttons = descendants::<gtk::Button>(app.widgets().panes[0].breadcrumb_box.upcast_ref());
    let button = buttons
        .into_iter()
        .find(|button| button.tooltip_text().as_deref() == Some(&path.to_string()))
        .expect("breadcrumb for path");
    button.grab_focus();
    assert_window_defers(app, gdk::Key::Return);
    assert_window_defers(app, gdk::Key::space);
    button.emit_clicked();
}

fn assert_window_defers(app: &relm4::Controller<AppModel>, key: gdk::Key) {
    let controllers = app.widget().observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(controller) = controllers
            .item(index)
            .and_downcast::<gtk::EventControllerKey>()
        {
            assert!(
                !controller.emit_by_name::<bool>(
                    "key-pressed",
                    &[&key, &0_u32, &gdk::ModifierType::empty()]
                ),
                "Window shortcuts must leave breadcrumb keyboard actions to GTK"
            );
        }
    }
}

fn descendants<T: IsA<gtk::Widget> + glib::object::IsClass>(widget: &gtk::Widget) -> Vec<T> {
    let mut result = Vec::new();
    if let Ok(found) = widget.clone().downcast::<T>() {
        result.push(found);
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        result.extend(descendants(&widget));
    }
    result
}

#[track_caller]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "breadcrumb condition timed out");
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn drain() {
    let deadline = Instant::now() + Duration::from_millis(180);
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(app: &relm4::Controller<AppModel>, name: &str) {
    let Some(directory) = std::env::var_os("COMMANDER_BREADCRUMB_SNAPSHOT_DIR") else {
        return;
    };
    let child = gtk::prelude::GtkWindowExt::child(app.widget()).unwrap();
    let snapshot = gtk::Snapshot::new();
    child.parent().unwrap().snapshot_child(&child, &snapshot);
    app.widget()
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}
