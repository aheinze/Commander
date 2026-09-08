use super::*;
use relm4::{Component, ComponentController};

#[test]
fn links_resolve_relative_paths_and_reject_executable_schemes() {
    let base = "file:///tmp/My%20Project/README.md";
    assert_eq!(
        safe_link(base, "docs/guide.md").as_deref(),
        Some("file:///tmp/My%20Project/docs/guide.md")
    );
    assert_eq!(safe_link(base, "#h%C3%A9llo").as_deref(), Some("#héllo"));
    assert_eq!(
        safe_link(base, "https://example.com/docs").as_deref(),
        Some("https://example.com/docs")
    );
    for link in [
        "javascript:alert(1)",
        "data:text/html,hello",
        "command:run",
        "ssh://host/",
    ] {
        assert!(safe_link(base, link).is_none(), "{link}");
    }
    assert_eq!(heading_slug("Héllo, Rust & GTK!"), "héllo-rust--gtk");
}

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_markdown_previews_format_documents_and_switch_back_to_source() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("a.md");
    let source = "# Markdown preview\n\nRead **bold**, _italic_, ~~removed~~ and `inline code`. [Jump to details](#details).\n\n## A readable document\n\n- First item\n  - A nested item\n- Second item\n\n- [x] Finished task\n- [ ] Next task\n\n> A quotation with **emphasis**.\n\n```rust\nfn main() {\n    println!(\"Hello, Markdown!\");\n}\n```\n\n| Feature | Status |\n| :--- | ---: |\n| Native tables | Ready |\n| Unicode | Héllo 🦀 |\n\n![Local illustration](inline.png)\n\n".to_owned() + &"Another paragraph with a [relative link](z.rs).\n\n".repeat(12) + "## Details\n\nThe anchor lands here.\n";
    std::fs::write(&path, source).unwrap();
    std::fs::write(fixture.path().join("z.rs"), "fn plain_source() {}\n").unwrap();
    image::RgbaImage::from_pixel(96, 32, image::Rgba([82, 139, 255, 255]))
        .save(fixture.path().join("inline.png"))
        .unwrap();
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
            == Some("markdown")
    });
    {
        let widgets = app.widgets();
        let root = &widgets.preview.markdown_view.content;
        let labels = descendants::<gtk::Label>(root.upcast_ref());
        assert!(
            labels
                .iter()
                .any(|label| label.text() == "Markdown preview")
        );
        assert!(
            labels
                .iter()
                .any(|label| label.label().contains("<b>bold</b>"))
        );
        assert!(!descendants::<gtk::Grid>(root.upcast_ref()).is_empty());
        assert_eq!(descendants::<gtk::Picture>(root.upcast_ref()).len(), 1);
        let code = descendants::<gtk::TextView>(root.upcast_ref())
            .pop()
            .unwrap()
            .buffer();
        assert!(
            code.text(&code.start_iter(), &code.end_iter(), false)
                .contains("fn main()")
        );
    }
    snapshot(app.widget(), "markdown-inspector-dark");
    {
        let label =
            descendants::<gtk::Label>(app.widgets().preview.markdown_view.content.upcast_ref())
                .into_iter()
                .find(|label| label.text().contains("Read bold"))
                .unwrap();
        label.grab_focus();
        label.select_region(0, 9);
        let controllers = app.widget().observe_controllers();
        for index in 0..controllers.n_items() {
            if let Some(keys) = controllers
                .item(index)
                .and_downcast::<gtk::EventControllerKey>()
            {
                for (key, modifiers) in [
                    (gdk::Key::c, gdk::ModifierType::CONTROL_MASK),
                    (gdk::Key::Right, gdk::ModifierType::SHIFT_MASK),
                ] {
                    assert!(!keys.emit_by_name::<bool>("key-pressed", &[&key, &0_u32, &modifiers]));
                }
            }
        }
    }
    app.emit(AppMsg::ToggleQuickLook);
    wait_until(|| {
        app.widgets()
            .quick_look
            .stack
            .visible_child_name()
            .as_deref()
            == Some("markdown")
    });
    snapshot(app.widget(), "markdown-quick-look-dark");
    {
        let widgets = app.widgets();
        let view = &widgets.quick_look.markdown_view;
        let picture = descendants::<gtk::Picture>(view.content.upcast_ref())
            .pop()
            .unwrap();
        assert!(
            picture.width() > 0 && picture.height() > 0,
            "Local image must have a visible allocation"
        );
        let bounds = picture.compute_bounds(&view.root).unwrap();
        view.root
            .vadjustment()
            .set_value(f64::from(bounds.y()) - 40.0);
    }
    snapshot(app.widget(), "markdown-local-image-dark");
    app.widgets()
        .quick_look
        .markdown_view
        .root
        .vadjustment()
        .set_value(0.0);
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "markdown-quick-look-light");
    {
        let widgets = app.widgets();
        let view = &widgets.quick_look.markdown_view;
        let link = descendants::<gtk::Label>(view.content.upcast_ref())
            .into_iter()
            .find(|label| label.text().contains("Jump to details"))
            .unwrap();
        assert!(link.emit_by_name::<bool>("activate-link", &[&"#details"]));
        assert!(view.root.vadjustment().value() > 0.0);
    }
    app.emit(AppMsg::ToggleQuickLook);
    wait_until(|| !app.model().quick_look_open);
    std::fs::write(&path, "# Updated document\n\nLive Markdown refresh.").unwrap();
    wait_until(|| {
        descendants::<gtk::Label>(app.widgets().preview.markdown_view.content.upcast_ref())
            .iter()
            .any(|label| label.text() == "Updated document")
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
            .contains("fn plain_source()")
    );
    app.widget().destroy();
}

fn descendants<T: IsA<gtk::Widget> + glib::object::IsClass>(root: &gtk::Widget) -> Vec<T> {
    let mut result = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        if let Ok(item) = widget.clone().downcast::<T>() {
            result.push(item);
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            pending.push(current.clone());
            child = current.next_sibling();
        }
    }
    result
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !ready() {
        assert!(Instant::now() < deadline, "Markdown preview timed out");
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    fn redraw(widget: &gtk::Widget) {
        widget.queue_draw();
        let mut child = widget.first_child();
        while let Some(current) = child {
            redraw(&current);
            child = current.next_sibling();
        }
    }
    redraw(window.upcast_ref());
    let deadline = Instant::now() + Duration::from_millis(250);
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
