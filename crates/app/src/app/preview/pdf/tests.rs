use super::*;
use lopdf::{Document, Object, dictionary};
use relm4::{Component, ComponentController};

#[test]
fn pdf_layout_keeps_mixed_pages_aligned_and_selects_nearby_pages() {
    let pages = [
        PageSize {
            width: 300.0,
            height: 600.0,
        },
        PageSize {
            width: 600.0,
            height: 300.0,
        },
    ];
    let scale = fitted_scale(&pages, 324);
    assert_eq!(scale, 0.5);
    let layout = Layout::new(&pages, scale, 324);
    assert_eq!(layout.widths, [150, 300]);
    assert_eq!(layout.tops, [12.0, 324.0]);
    assert_eq!(layout.page_at(400.0), 1);
    assert_eq!(layout.nearby(400.0, 50.0), 1..2);
}

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_pdf_scrolls_navigates_zooms_and_switches_documents() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    write_pdf(&fixture.path().join("a-reader.pdf"), 120);
    write_pdf(&fixture.path().join("b-one-page.pdf"), 1);
    std::fs::write(fixture.path().join("z.txt"), "A plain text preview").unwrap();
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
            .pdf_view
            .inner
            .document
            .borrow()
            .as_ref()
            .is_some_and(|document| document.slots.get(&0).is_some_and(|slot| slot.ready))
    });
    let inspector = Rc::clone(&app.widgets().preview.pdf_view.inner);
    assert_eq!(inspector.count.text(), "/ 120");
    assert!(inspector.fit.is_active());
    assert!(!inspector.previous.is_sensitive());
    assert!(inspector.document.borrow().as_ref().unwrap().slots.len() < 8);
    assert!(
        inspector.scroll.width() < 400,
        "toolbar must not widen the inspector"
    );
    snapshot(app.widget(), "pdf-inspector-dark");
    inspector.scroll.vadjustment().set_value(600.0);
    wait_until(|| inspector.document.borrow().as_ref().unwrap().current > 0);
    inspector.page.set_text("80");
    inspector.page.emit_activate();
    wait_until(|| inspector.document.borrow().as_ref().unwrap().current == 79);
    wait_until(|| {
        inspector
            .document
            .borrow()
            .as_ref()
            .unwrap()
            .slots
            .get(&79)
            .is_some_and(|slot| slot.ready)
    });
    let cached = inspector
        .document
        .borrow()
        .as_ref()
        .unwrap()
        .cache
        .iter()
        .find(|page| page.index == 79)
        .unwrap()
        .texture
        .clone();
    inspector.go_to(70);
    wait_until(|| inspector.document.borrow().as_ref().unwrap().current == 70);
    inspector.go_to(79);
    wait_until(|| inspector.document.borrow().as_ref().unwrap().current == 79);
    assert_eq!(
        inspector
            .document
            .borrow()
            .as_ref()
            .unwrap()
            .cache
            .iter()
            .find(|page| page.index == 79)
            .unwrap()
            .texture,
        cached
    );
    // Navigation shortcuts are owned by the document while it has keyboard focus.
    inspector.scroll.grab_focus();
    let controllers = app.widget().observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(keys) = controllers
            .item(index)
            .and_downcast::<gtk::EventControllerKey>()
        {
            for (key, modifiers) in [
                (gdk::Key::Down, gdk::ModifierType::empty()),
                (gdk::Key::l, gdk::ModifierType::CONTROL_MASK),
            ] {
                assert!(!keys.emit_by_name::<bool>("key-pressed", &[&key, &0_u32, &modifiers]));
            }
        }
    }
    // Rapid zoom and page changes replace obsolete requests, preserving the latest target.
    inspector.zoom_by(1);
    inspector.zoom_by(1);
    inspector.go_to(99);
    wait_until(|| inspector.document.borrow().as_ref().unwrap().current == 99);
    wait_until(|| {
        inspector
            .document
            .borrow()
            .as_ref()
            .unwrap()
            .slots
            .get(&99)
            .is_some_and(|slot| slot.ready)
    });
    assert!(!inspector.fit.is_active());
    assert!(inspector.document.borrow().as_ref().unwrap().cache_bytes <= CACHE_BYTES);
    inspector.go_to(usize::MAX);
    wait_until(|| inspector.document.borrow().as_ref().unwrap().current == 119);
    let bookmark = inspector
        .outline
        .first_child()
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    bookmark.emit_clicked();
    wait_until(|| inspector.document.borrow().as_ref().unwrap().current == 4);
    inspector.fit_width();
    wait_until(|| inspector.fit.is_active());
    app.emit(AppMsg::ToggleQuickLook);
    wait_until(|| {
        app.widgets()
            .quick_look
            .stack
            .visible_child_name()
            .as_deref()
            == Some("pdf")
    });
    let reader = Rc::clone(&app.widgets().quick_look.pdf_view.inner);
    wait_until(|| {
        reader
            .document
            .borrow()
            .as_ref()
            .unwrap()
            .slots
            .get(&0)
            .is_some_and(|slot| slot.ready)
    });
    reader.scroll.grab_focus();
    assert_eq!(
        reader.key(gdk::Key::Page_Down, gdk::ModifierType::empty()),
        glib::Propagation::Stop
    );
    wait_until(|| reader.scroll.vadjustment().value() > 0.0);
    reader.go_to(4);
    wait_until(|| reader.document.borrow().as_ref().unwrap().current == 4);
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "pdf-quick-look-light");
    // A file switch while rendering cannot put pages from the old document in the new one.
    reader.zoom_by(1);
    app.emit(AppMsg::MoveCursor(1, false));
    wait_until(|| {
        reader
            .document
            .borrow()
            .as_ref()
            .is_some_and(|document| document.source.pages.len() == 1)
    });
    wait_until(|| {
        reader
            .document
            .borrow()
            .as_ref()
            .unwrap()
            .slots
            .get(&0)
            .is_some_and(|slot| slot.ready)
    });
    assert!(!reader.next.is_sensitive());
    assert!(!reader.bookmarks.is_sensitive());
    assert_eq!(reader.page.text(), "1");
    app.emit(AppMsg::ToggleQuickLook);
    wait_until(|| !app.model().quick_look_open);
    assert!(reader.document.borrow().is_none());
    app.emit(AppMsg::MoveCursor(1, false));
    wait_until(|| {
        app.widgets()
            .preview
            .content_stack
            .visible_child_name()
            .as_deref()
            == Some("text")
    });
    assert!(inspector.document.borrow().is_none());
    app.widget().destroy();
}

fn write_pdf(path: &std::path::Path, count: usize) {
    let mut pdf = Document::with_version("1.4");
    let parent = pdf.new_object_id();
    let font = pdf.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let mut pages = Vec::new();
    for index in 0..count {
        let (width, height) = if index % 3 == 2 {
            (792, 612)
        } else {
            (612, 792)
        };
        let content = format!(
            "0.12 0.25 0.45 rg 40 {} 532 80 re f BT /F1 24 Tf 1 1 1 rg 60 {} Td (Commander document preview) Tj ET BT /F1 16 Tf 0.15 0.15 0.15 rg 40 {} Td (Page {} - Project notes) Tj ET BT /F1 12 Tf 40 {} Td (Scroll continuously. Jump to a page. Keep your place while zooming.) Tj ET",
            height - 130,
            height - 100,
            height - 180,
            index + 1,
            height - 215
        );
        let stream = pdf.add_object(lopdf::Stream::new(
            lopdf::Dictionary::new(),
            content.into_bytes(),
        ));
        pages.push(pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => parent,
            "MediaBox" => vec![0.into(), 0.into(), width.into(), height.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } }, "Contents" => stream,
        }));
    }
    pdf.objects.insert(
        parent,
        dictionary! { "Type" => "Pages", "Count" => count as i64,
        "Kids" => pages.iter().map(|&id| Object::Reference(id)).collect::<Vec<_>>() }
        .into(),
    );
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => parent };
    if count > 4 {
        let outlines = pdf.new_object_id();
        let bookmark = pdf.add_object(dictionary! {
            "Title" => Object::string_literal("Project notes"), "Parent" => outlines,
            "Dest" => vec![Object::Reference(pages[4]), Object::Name(b"Fit".to_vec())]
        });
        pdf.objects.insert(
            outlines,
            dictionary! { "First" => bookmark, "Last" => bookmark, "Count" => 1 }.into(),
        );
        catalog.set("Outlines", outlines);
    }
    let root = pdf.add_object(catalog);
    pdf.trailer.set("Root", root);
    pdf.save(path).unwrap();
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "PDF preview timed out");
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(350);
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
