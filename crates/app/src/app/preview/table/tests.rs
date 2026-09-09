use super::*;
use relm4::{Component, ComponentController};
use std::io::Write;

#[test]
fn worksheet_column_names_preserve_excel_coordinates() {
    for (index, expected) in [
        (0, "A"),
        (25, "Z"),
        (26, "AA"),
        (49, "AX"),
        (701, "ZZ"),
        (16383, "XFD"),
    ] {
        assert_eq!(column_name(index), expected);
    }
}

#[test]
#[ignore = "requires an isolated GTK display; run in the native suite"]
fn gtk_table_previews_scroll_switch_sheets_and_refresh() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let csv = fixture.path().join("a-sales.csv");
    let mut content = "Product,Region,Units,Revenue,Status,Notes\n".to_owned();
    for row in 1..=80 {
        content.push_str(&format!(
            "Studio display {row},Europe,{},{}.50,Shipped,Quarterly order\n",
            row * 2,
            row * 249
        ));
    }
    std::fs::write(&csv, content).unwrap();
    workbook(&fixture.path().join("b-workbook.xlsx"));
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
            == Some("table")
    });
    {
        let widgets = app.widgets();
        let view = &widgets.preview.table_view.inner;
        assert_eq!(view.table.columns().n_items(), 7);
        assert_eq!(view.table.model().unwrap().n_items(), 81);
        assert!(!view.sheet_bar.is_visible());
    }
    snapshot(app.widget(), "csv-inspector-dark");
    {
        let root = app.widgets().preview.table_view.root.clone();
        let mut pending = vec![root.upcast::<gtk::Widget>()];
        let mut cell = None;
        while let Some(widget) = pending.pop() {
            if let Some(label) = widget.downcast_ref::<gtk::Label>()
                && label.is_selectable()
                && label.text() == "Product"
            {
                cell = Some(label.clone());
                break;
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                pending.push(widget);
            }
        }
        let cell = cell.expect("visible table cell");
        assert!(cell.grab_focus());
        cell.select_region(0, 7);
        let controllers = app.widget().observe_controllers();
        for index in 0..controllers.n_items() {
            if let Some(keys) = controllers
                .item(index)
                .and_downcast::<gtk::EventControllerKey>()
            {
                for (key, modifiers) in [
                    (gdk::Key::c, gdk::ModifierType::CONTROL_MASK),
                    (gdk::Key::Right, gdk::ModifierType::SHIFT_MASK),
                    (gdk::Key::Down, gdk::ModifierType::empty()),
                ] {
                    assert!(!keys.emit_by_name::<bool>("key-pressed", &[&key, &0_u32, &modifiers]));
                }
            }
        }
    }
    {
        let widgets = app.widgets();
        let view = &widgets.preview.table_view.inner;
        assert!(view.scroll.hadjustment().upper() > view.scroll.hadjustment().page_size());
        assert!(view.scroll.vadjustment().upper() > view.scroll.vadjustment().page_size());
        view.scroll.hadjustment().set_value(180.0);
        view.scroll.vadjustment().set_value(180.0);
    }
    std::fs::write(&csv, "Name,Value\nUpdated,42\n").unwrap();
    wait_until(|| {
        app.widgets()
            .preview
            .table_view
            .inner
            .table
            .model()
            .is_some_and(|model| model.n_items() == 2)
    });
    app.emit(AppMsg::MoveCursor(1, false));
    wait_until(|| {
        app.widgets()
            .preview
            .table_view
            .inner
            .sheet_bar
            .is_visible()
    });
    app.emit(AppMsg::ToggleQuickLook);
    wait_until(|| {
        app.widgets()
            .quick_look
            .stack
            .visible_child_name()
            .as_deref()
            == Some("table")
    });
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "excel-quick-look-light");
    {
        let widgets = app.widgets();
        let view = &widgets.quick_look.table_view.inner;
        assert_eq!(view.sheets.model().unwrap().n_items(), 2);
        assert_eq!(view.table.model().unwrap().n_items(), 41);
        view.sheets.set_selected(1);
        assert_eq!(view.table.model().unwrap().n_items(), 2);
        assert_eq!(
            view.document.borrow().as_ref().unwrap().sheets[1].rows[1][0],
            "München"
        );
        assert_eq!(view.scroll.hadjustment().value(), 0.0);
        let empty = TableDocument {
            format: "Excel",
            sheets: vec![TableSheet {
                name: "Empty".into(),
                ..TableSheet::default()
            }],
            truncated: false,
        };
        widgets.quick_look.table_view.render(&empty);
        assert!(view.empty.is_visible());
        assert_eq!(view.empty.label(), "This sheet is empty.");
    }
    app.emit(AppMsg::ToggleQuickLook);
    wait_until(|| !app.model().quick_look_open);
    app.emit(AppMsg::MoveCursorTo(CursorTarget::Last, false));
    wait_until(|| {
        app.widgets()
            .preview
            .content_stack
            .visible_child_name()
            .as_deref()
            == Some("text")
    });
    assert!(
        app.widgets()
            .preview
            .table_view
            .inner
            .document
            .borrow()
            .is_none()
    );
    app.widget().destroy();
}

fn workbook(path: &std::path::Path) {
    let mut sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:E41"/><sheetData><row r="1">"#.to_owned();
    for (col, text) in ["Product", "Region", "Units", "Revenue", "Status"]
        .iter()
        .enumerate()
    {
        sheet.push_str(&format!(
            r#"<c r="{}1" t="inlineStr"><is><t>{text}</t></is></c>"#,
            column_name(col as u32)
        ));
    }
    sheet.push_str("</row>");
    for row in 2..=41 {
        sheet.push_str(&format!(r#"<row r="{row}"><c r="A{row}" t="inlineStr"><is><t>Studio display {row}</t></is></c><c r="B{row}" t="inlineStr"><is><t>Europe</t></is></c><c r="C{row}"><v>{}</v></c><c r="D{row}"><v>{}.50</v></c><c r="E{row}" t="inlineStr"><is><t>Shipped</t></is></c></row>"#, row * 2, row * 249));
    }
    sheet.push_str("</sheetData></worksheet>");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, content) in [
        (
            "_rels/.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="xl/workbook.xml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sales" sheetId="1" r:id="rId1"/><sheet name="Locations" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.xml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/><Relationship Id="rId2" Target="worksheets/sheet2.xml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>"#,
        ),
        ("xl/worksheets/sheet1.xml", &sheet),
        (
            "xl/worksheets/sheet2.xml",
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:B2"/><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>City</t></is></c><c r="B1" t="inlineStr"><is><t>Country</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>München</t></is></c><c r="B2" t="inlineStr"><is><t>Germany</t></is></c></row></sheetData></worksheet>"#,
        ),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(Instant::now() < deadline, "Table preview timed out");
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
