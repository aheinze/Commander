//! Shared virtualized, read-only tables for the inspector and Quick Look.

use super::super::*;
use dualpane_thumbs::table::{TableDocument, TableSheet};

#[cfg(test)]
mod tests;

pub(super) struct TableView {
    pub root: gtk::Box,
    inner: Rc<View>,
}

struct View {
    sheets: gtk::DropDown,
    sheet_bar: gtk::Box,
    table: gtk::ColumnView,
    scroll: gtk::ScrolledWindow,
    status: gtk::Label,
    empty: gtk::Label,
    document: RefCell<Option<TableDocument>>,
}

struct Row {
    sheet: Rc<TableSheet>,
    index: usize,
}

impl TableView {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("table-preview");
        let sheet_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        sheet_bar.add_css_class("table-preview-toolbar");
        let label = gtk::Label::new(Some("Sheet"));
        let sheets = gtk::DropDown::from_strings(&[]);
        sheets.set_hexpand(true);
        sheets.set_enable_search(true);
        sheets.set_tooltip_text(Some("Choose worksheet"));
        sheets.update_property(&[gtk::accessible::Property::Label("Worksheet")]);
        sheet_bar.append(&label);
        sheet_bar.append(&sheets);
        root.append(&sheet_bar);
        let table = gtk::ColumnView::new(None::<gtk::SelectionModel>);
        table.set_show_row_separators(true);
        table.set_show_column_separators(true);
        table.add_css_class("data-table");
        table.update_property(&[gtk::accessible::Property::Label("Spreadsheet preview")]);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&table)
            .build();
        scroll.set_overlay_scrolling(false);
        root.append(&scroll);
        let empty = gtk::Label::new(None);
        empty.set_wrap(true);
        empty.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        empty.set_max_width_chars(36);
        empty.set_vexpand(true);
        empty.add_css_class("table-preview-empty");
        root.append(&empty);
        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        status.set_max_width_chars(36);
        status.add_css_class("table-preview-status");
        root.append(&status);
        let inner = Rc::new(View {
            sheets,
            sheet_bar,
            table,
            scroll,
            status,
            empty,
            document: RefCell::new(None),
        });
        let weak = Rc::downgrade(&inner);
        inner.sheets.connect_selected_notify(move |_| {
            if let Some(view) = weak.upgrade() {
                view.show_sheet();
            }
        });
        Self { root, inner }
    }

    pub fn render(&self, document: &TableDocument) {
        let names = document
            .sheets
            .iter()
            .map(|sheet| sheet.name.as_str())
            .collect::<Vec<_>>();
        // Clear before replacing the selector model, whose notify can fire immediately.
        self.inner.document.replace(None);
        self.inner
            .sheets
            .set_model(Some(&gtk::StringList::new(&names)));
        self.inner.sheets.set_selected(0);
        self.inner
            .sheet_bar
            .set_visible(document.format == "Excel" && !names.is_empty());
        self.inner.document.replace(Some(document.clone()));
        self.inner.show_sheet();
    }

    pub fn clear(&self) {
        self.inner.document.replace(None);
        self.inner.table.set_model(gtk::SelectionModel::NONE);
    }
}

impl View {
    fn show_sheet(&self) {
        self.table.set_model(gtk::SelectionModel::NONE);
        while let Some(column) = self
            .table
            .columns()
            .item(0)
            .and_downcast::<gtk::ColumnViewColumn>()
        {
            self.table.remove_column(&column);
        }
        let document = self.document.borrow();
        let Some(document) = document.as_ref() else {
            return;
        };
        let Some(sheet) = document.sheets.get(self.sheets.selected() as usize) else {
            self.empty.set_label("This workbook has no worksheets.");
            self.empty.set_visible(true);
            self.scroll.set_visible(false);
            self.status.set_label("Read-only preview");
            return;
        };
        let is_empty = sheet.rows.is_empty() || sheet.error.is_some();
        self.scroll.set_visible(!is_empty);
        self.empty.set_visible(is_empty);
        if let Some(error) = &sheet.error {
            notifications::error(error);
        }
        self.empty.set_label(if sheet.error.is_some() {
            ""
        } else if sheet.truncated {
            "No cells within the preview limits. Open the file to see more."
        } else {
            "This sheet is empty."
        });
        let status = format!("{} rows · {} columns", sheet.rows.len(), sheet.columns);
        if sheet.truncated {
            notifications::info("Preview limited. Open the file to see all data.");
        }
        if document.truncated {
            notifications::info("Only the first 20 sheets are shown in the preview.");
        }
        self.status.set_label(&status);
        if is_empty {
            return;
        }
        append_column(&self.table, "#", None);
        for column in 0..sheet.columns {
            append_column(
                &self.table,
                &column_name(sheet.first_column + column as u32),
                Some(column),
            );
        }
        let sheet = Rc::new(sheet.clone());
        let rows = gio::ListStore::new::<glib::BoxedAnyObject>();
        for index in 0..sheet.rows.len() {
            rows.append(&glib::BoxedAnyObject::new(Row {
                sheet: Rc::clone(&sheet),
                index,
            }));
        }
        self.table
            .set_model(Some(&gtk::NoSelection::new(Some(rows))));
        self.scroll.hadjustment().set_value(0.0);
        self.scroll.vadjustment().set_value(0.0);
    }
}

fn append_column(table: &gtk::ColumnView, title: &str, index: Option<usize>) {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let label = gtk::Label::new(None);
        label.set_selectable(index.is_some());
        label.set_single_line_mode(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_width_chars(1);
        label.set_max_width_chars(1);
        label.add_css_class(if index.is_some() {
            "table-preview-cell"
        } else {
            "table-preview-row-number"
        });
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let object = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
        let row = object.borrow::<Row>();
        let text = index.map_or_else(
            || (row.sheet.first_row + row.index as u32 + 1).to_string(),
            |col| {
                row.sheet.rows[row.index]
                    .get(col)
                    .cloned()
                    .unwrap_or_default()
            },
        );
        let label = item.child().and_downcast::<gtk::Label>().unwrap();
        label.set_label(&text);
        label.set_tooltip_text((!text.is_empty()).then_some(text.as_str()));
        label.set_xalign(if index.is_none() || text.parse::<f64>().is_ok() {
            1.0
        } else {
            0.0
        });
        let coordinate = format!(
            "{}{}: {text}",
            index.map_or_else(String::new, |col| column_name(
                row.sheet.first_column + col as u32
            )),
            row.sheet.first_row + row.index as u32 + 1
        );
        label.update_property(&[gtk::accessible::Property::Label(&coordinate)]);
    });
    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_fixed_width(if index.is_some() { 156 } else { 48 });
    column.set_resizable(index.is_some());
    table.append_column(&column);
}

fn column_name(mut index: u32) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap()
}
