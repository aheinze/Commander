use super::*;
use relm4::{Component, ComponentController};

fn settle() {
    let deadline = Instant::now() + Duration::from_millis(750);
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn snapshot(app: &relm4::Controller<AppModel>, name: &str) {
    let Some(directory) = std::env::var_os("COMMANDER_TEST_ARTIFACTS") else {
        return;
    };
    let snapshot = gtk::Snapshot::new();
    app.widget().snapshot_child(
        &gtk::prelude::GtkWindowExt::child(app.widget()).unwrap(),
        &snapshot,
    );
    app.widget()
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(std::path::Path::new(&directory).join(format!("snapshots/{name}.png")))
        .unwrap();
}

fn assert_two_panels(app: &relm4::Controller<AppModel>) {
    let widgets = app.widgets();
    let horizontal = widgets.paned.orientation() == gtk::Orientation::Horizontal;
    let dimensions: Vec<_> = widgets
        .panes
        .iter()
        .map(|pane| {
            (
                pane.root.is_visible(),
                pane.root.width(),
                pane.root.height(),
            )
        })
        .collect();
    assert!(app.model().dual_pane);
    let minimum = if horizontal { 200 } else { 120 };
    assert!(
        widgets.paned.position() >= minimum
            && widgets.paned.max_position() - widgets.paned.position() >= minimum
            && dimensions.iter().all(|(visible, width, height)| *visible
                && if horizontal {
                    *width >= 200
                } else {
                    *height >= 120
                }),
        "Both enabled file panels must remain usable: {dimensions:?}; orientation={:?}, split={} / {}, workspace={} / {}",
        widgets.paned.orientation(),
        widgets.paned.position(),
        widgets.paned.max_position(),
        widgets.workspace_paned.position(),
        widgets.workspace_paned.width()
    );
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_layout_restores_two_panels_with_inspector_and_constrains_resizes() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("Left files");
    let right = fixture.path().join("Right files");
    for folder in [&left, &right] {
        std::fs::create_dir(folder).unwrap();
        for name in ["Documents", "Projects", "Pictures"] {
            std::fs::create_dir(folder.join(name)).unwrap();
        }
    }
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(left)),
                right: Some(VPath::from(right)),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                dual_pane: true,
                vertical_split: false,
                split_position: 905,
                sidebar_visible: true,
                preview_visible: true,
                preview_width: 560,
                window_width: 1767,
                window_height: 988,
                ..SessionState::default()
            }),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    settle();
    snapshot(&app, "layout-restored");
    assert_two_panels(&app);
    // A late allocation or stale restored divider can arrive after the startup
    // timers. Both panels are enabled, so an edge position must remain usable.
    let paned = app.widgets().paned.clone();
    paned.set_position(paned.max_position());
    settle();
    snapshot(&app, "layout-late-divider");
    assert_two_panels(&app);
    // Valid user-chosen proportions survive; resizing cannot collapse a pane.
    let chosen = paned.max_position() * 3 / 5;
    paned.set_position(chosen);
    settle();
    assert_eq!(paned.position(), chosen);
    app.widget().set_default_size(1280, 860);
    settle();
    assert_two_panels(&app);
    assert_eq!(paned.orientation(), gtk::Orientation::Vertical);
    snapshot(&app, "layout-narrow-inspector");
    paned.set_position(0);
    settle();
    assert_two_panels(&app);
    app.emit(AppMsg::TogglePreview);
    settle();
    assert_two_panels(&app);
    app.emit(AppMsg::TogglePreview);
    settle();
    assert_two_panels(&app);
    app.emit(AppMsg::ToggleDualPane);
    settle();
    assert!(!app.model().dual_pane);
    assert!(!app.widgets().panes[1].root.is_visible());
    app.emit(AppMsg::ToggleDualPane);
    settle();
    assert_two_panels(&app);
    app.emit(AppMsg::ExecuteCommand(CommandId::ToggleSidebar));
    app.widget().set_default_size(1767, 988);
    settle();
    assert_two_panels(&app);
    snapshot(&app, "layout-restored-again");
    app.widget().close();
}
