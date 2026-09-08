use super::*;
use relm4::{Component, ComponentController};

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_svg_previews_render_images_refresh_and_switch_back_to_source() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("a.svg");
    std::fs::write(
        &path,
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/branding/commander.svg"
        )),
    )
    .unwrap();
    std::fs::write(fixture.path().join("z.rs"), "fn source_preview() {}\n").unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(fixture.path())),
                right: Some(VPath::from(fixture.path())),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                dual_pane: false,
                sidebar_visible: false,
                preview_visible: true,
                window_width: 1120,
                window_height: 860,
                appearance: AppearanceMode::Dark,
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
        app.widgets()
            .preview
            .content_stack
            .visible_child_name()
            .as_deref()
            == Some("image")
    });
    assert!(app.widgets().preview.picture.paintable().is_some());
    snapshot(app.widget(), "svg-inspector-dark");
    app.emit(AppMsg::ToggleQuickLook);
    wait_until(|| {
        app.widgets()
            .quick_look
            .stack
            .visible_child_name()
            .as_deref()
            == Some("image")
    });
    assert!(app.widgets().quick_look.picture.paintable().is_some());
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "svg-quick-look-light");
    app.emit(AppMsg::ToggleQuickLook);
    wait_until(|| !app.model().quick_look_open);
    std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10"><rect width="20" height="10" fill="red"/></svg>"#).unwrap();
    wait_until(|| {
        app.widgets()
            .preview
            .picture
            .paintable()
            .is_some_and(|image| image.intrinsic_height() == 800)
    });
    app.emit(AppMsg::MoveCursorTo(CursorTarget::Last, false));
    wait_until(|| {
        app.widgets()
            .preview
            .content_stack
            .visible_child_name()
            .as_deref()
            == Some("text")
    });
    let buffer = app.widgets().preview.text_view.buffer();
    assert!(
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .contains("fn source_preview()")
    );
    app.widget().destroy();
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(Instant::now() < deadline, "SVG preview timed out");
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(300);
    window.queue_draw();
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
    let child = gtk::prelude::GtkWindowExt::child(window).unwrap();
    let snapshot = gtk::Snapshot::new();
    window.snapshot_child(&child, &snapshot);
    window
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(
            std::path::Path::new(&std::env::var_os("COMMANDER_TEST_ARTIFACTS").unwrap())
                .join("snapshots")
                .join(format!("{name}.png")),
        )
        .unwrap();
}
