//! Shared native tables and cancellable utility workers.
use super::*;
#[cfg(test)]
mod tests;

pub(super) fn text_table(store: &gio::ListStore, titles: &[&str]) -> gtk::ColumnView {
    let table = gtk::ColumnView::new(Some(gtk::NoSelection::new(Some(store.clone()))));
    table.add_css_class("power-tools-table");
    table.set_show_column_separators(false);
    for (index, title) in titles.iter().enumerate() {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::new(None);
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            label.add_css_class("power-tools-cell");
            item.set_child(Some(&label));
        });
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let object = item
                .item()
                .unwrap()
                .downcast::<glib::BoxedAnyObject>()
                .unwrap();
            let values = object.borrow::<Vec<String>>();
            let label = item.child().unwrap().downcast::<gtk::Label>().unwrap();
            let text = values.get(index).map_or("", String::as_str);
            label.set_text(text);
            label.set_tooltip_text(Some(text));
            let changed = values
                .last()
                .is_some_and(|s| matches!(s.as_str(), "Changed" | "Removed" | "Added"));
            if changed {
                label.add_css_class("power-tools-changed");
            } else {
                label.remove_css_class("power-tools-changed");
            }
        });
        let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
        column.set_resizable(true);
        let width = match *title {
            "Group" | "Left line" | "Right line" => Some(80),
            "Size" => Some(100),
            "Status" | "Difference" | "Pending change" => Some(120),
            _ => None,
        };
        column.set_expand(width.is_none());
        if let Some(width) = width {
            column.set_fixed_width(width);
        }
        table.append_column(&column);
    }
    table
}

/// Match the utility dialogs: inset body and a separate, full-width action footer.
pub(super) fn layout(view: &adw::ToolbarView) -> (gtk::Box, gtk::Box) {
    view.add_css_class("file-tools");
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("dialog-body");
    body.set_vexpand(true);
    let actions = dialogs::dialog_actions();
    page.append(&body);
    page.append(&actions);
    view.set_content(Some(&page));
    (body, actions)
}

pub(super) fn field(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    child
        .as_ref()
        .update_property(&[gtk::accessible::Property::Label(title)]);
    if let Some(entry) = child.as_ref().downcast_ref::<gtk::Entry>() {
        entry.set_width_chars(1);
        entry.set_activates_default(true);
    }
    let row = dialogs::form_row(title, child);
    row.add_css_class("file-tool-field");
    row
}

pub(super) fn button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("dialog-button");
    button
}

pub(super) fn cancel_button(dialog: &adw::Dialog) -> gtk::Button {
    let button = button("Cancel");
    dialog
        .bind_property("can-close", &button, "sensitive")
        .sync_create()
        .build();
    let dialog = dialog.downgrade();
    button.connect_clicked(move |_| {
        if let Some(dialog) = dialog.upgrade() {
            dialog.close();
        }
    });
    button
}

pub(super) fn summary(text: Option<&str>) -> gtk::Label {
    let label = gtk::Label::new(text);
    label.add_css_class("file-tool-summary");
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_selectable(true);
    label.set_can_focus(false);
    label
}

pub(super) fn table_scroll(table: &gtk::ColumnView, label: &str) -> gtk::ScrolledWindow {
    table.update_property(&[gtk::accessible::Property::Label(label)]);
    gtk::ScrolledWindow::builder()
        .child(table)
        .vexpand(true)
        .overlay_scrolling(false)
        .min_content_height(140)
        .build()
}

/// Only the UI thread owns widgets. Closing the sheet cancels and drops late results.
pub(super) fn background<T: Send + 'static>(
    _cancel: CancelToken,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    mut done: impl FnMut(Result<T, String>) + 'static,
) {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("commander-power-tool".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .unwrap_or_else(|_| Err("The worker stopped unexpectedly. Try again.".into()));
            let _ = tx.send(result);
        });
    if let Err(error) = worker {
        done(Err(format!("Could not start worker: {error}")));
        return;
    }
    glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
        Ok(result) => {
            done(result);
            glib::ControlFlow::Break
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(_) => {
            done(Err("The worker stopped unexpectedly".into()));
            glib::ControlFlow::Break
        }
    });
}

pub(super) fn duplicates(vfs: Arc<dyn Vfs>, folder: VPath, input: relm4::Sender<AppMsg>) {
    let Some(parent) = relm4::main_application().active_window() else {
        return;
    };
    let (dialog, view) =
        dialogs::utility_dialog("Find Duplicate Files", 860, 620, "duplicates-dialog");
    let (root, actions) = layout(&view);
    let feedback = notifications::Feedback::default();
    let location = gtk::Entry::new();
    location.set_text(&folder.to_string());
    root.append(&field("Folder", &location));
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let hidden = gtk::CheckButton::with_label("Include hidden files and folders");
    hidden.set_hexpand(true);
    toolbar.append(&hidden);
    let scan = button("Scan folder");
    scan.add_css_class("suggested-action");
    dialog.set_default_widget(Some(&scan));
    toolbar.append(&scan);
    let stop = button("Stop");
    stop.set_sensitive(false);
    stop.set_visible(false);
    toolbar.append(&stop);
    root.append(&toolbar);
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let table = text_table(&store, &["Group", "Size", "File path"]);
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    table.set_model(Some(&selection));
    root.append(&table_scroll(&table, "File results"));
    let status = summary(Some(
        "Scan compares file contents using SHA-256. Links are excluded. No files are deleted.",
    ));
    status.set_xalign(0.0);
    status.set_wrap(true);
    root.append(&status);
    let reveal = button("Show folder");
    reveal.set_sensitive(false);
    actions.append(&reveal);
    let paths = Rc::new(RefCell::new(Vec::<VPath>::new()));
    let cancel = Rc::new(RefCell::new(CancelToken::new()));
    let closed = Rc::new(Cell::new(false));
    {
        let cancel = cancel.clone();
        stop.connect_clicked(move |_| cancel.borrow().cancel());
    }
    {
        let cancel = cancel.clone();
        let closed = closed.clone();
        dialog.connect_closed(move |_| {
            closed.set(true);
            cancel.borrow().cancel();
        });
    }
    {
        let reveal = reveal.clone();
        selection.connect_selected_notify(move |s| {
            reveal.set_sensitive(s.selected() != gtk::INVALID_LIST_POSITION)
        });
    }
    {
        let selection = selection.clone();
        let paths = paths.clone();
        reveal.connect_clicked(move |_| {
            if let Some(path) = paths
                .borrow()
                .get(selection.selected() as usize)
                .and_then(VPath::parent)
            {
                let _ = input.send(AppMsg::NavigateActive(path));
            }
        });
    }
    scan.connect_clicked(move |scan| {
        let folder = VPath::from(location.text().as_str());
        if !folder.as_path().is_absolute() {
            location.add_css_class("error");
            feedback.validate(Some("Enter an absolute folder path."), &location);
            return;
        }
        location.remove_css_class("error");
        feedback.validate(None, &location);
        store.remove_all();
        paths.borrow_mut().clear();
        scan.set_sensitive(false);
        stop.set_sensitive(true);
        stop.set_visible(true);
        location.set_sensitive(false);
        hidden.set_sensitive(false);
        status.set_text("Scanning folders and comparing contents…");
        let token = CancelToken::new();
        *cancel.borrow_mut() = token.clone();
        let work_token = token.clone();
        let vfs = vfs.clone();
        let include_hidden = hidden.is_active();
        let store = store.clone();
        let paths = paths.clone();
        let status = status.clone();
        let scan = scan.clone();
        let stop = stop.clone();
        let closed = closed.clone();
        let feedback = feedback.clone();
        let location = location.clone();
        let hidden = hidden.clone();
        background(
            token,
            move || {
                crate::features::duplicates::find(
                    vfs.as_ref(),
                    &folder,
                    include_hidden,
                    &work_token,
                )
            },
            move |result| {
                if closed.get() {
                    return;
                }
                scan.set_sensitive(true);
                stop.set_sensitive(false);
                stop.set_visible(false);
                location.set_sensitive(true);
                hidden.set_sensitive(true);
                match result {
                    Ok(result) => {
                        let summary = format!(
                            "Groups: {} · Files scanned: {} · Redundant bytes: {} · Skipped: {}{}",
                            result.groups.len(),
                            result.scanned,
                            result.reclaimable(),
                            result.skipped,
                            if result.groups.is_empty() {
                                " · No duplicates found in scanned files"
                            } else {
                                ""
                            }
                        );
                        status.set_text(&summary);
                        status.set_tooltip_text(Some(&result.warnings.join("\n")));
                        for (index, group) in result.groups.iter().enumerate() {
                            for path in &group.paths {
                                store.append(&glib::BoxedAnyObject::new(vec![
                                    (index + 1).to_string(),
                                    format!("{} B", group.size),
                                    path.to_string(),
                                ]));
                                paths.borrow_mut().push(path.clone());
                            }
                        }
                    }
                    Err(error) => {
                        status.set_text("No results available. Adjust the options and try again.");
                        feedback.error(&error);
                    }
                }
            },
        );
    });
    dialog.present(Some(&parent));
}

pub(super) fn compare(vfs: Arc<dyn Vfs>, left: VPath, right: VPath) {
    let Some(parent) = relm4::main_application().active_window() else {
        return;
    };
    let (dialog, view) = dialogs::utility_dialog("Compare Files", 1040, 680, "file-compare-dialog");
    let (root, actions) = layout(&view);
    let feedback = notifications::Feedback::default();
    let left_path = gtk::Entry::new();
    left_path.set_text(&left.to_string());
    root.append(&field("Left file", &left_path));
    let right_path = gtk::Entry::new();
    right_path.set_text(&right.to_string());
    root.append(&field("Right file", &right_path));
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let case = gtk::CheckButton::with_label("Ignore case");
    let space = gtk::CheckButton::with_label("Ignore whitespace");
    let run = button("Compare");
    run.add_css_class("suggested-action");
    dialog.set_default_widget(Some(&run));
    let stop = button("Stop");
    stop.set_sensitive(false);
    stop.set_visible(false);
    for child in [
        case.upcast_ref::<gtk::Widget>(),
        space.upcast_ref(),
        run.upcast_ref(),
        stop.upcast_ref(),
    ] {
        toolbar.append(child);
    }
    root.append(&toolbar);
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let table = text_table(
        &store,
        &[
            "Left line",
            "Left contents",
            "Right line",
            "Right contents",
            "Difference",
        ],
    );
    table.add_css_class("file-compare-table");
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    table.set_model(Some(&selection));
    root.append(&table_scroll(&table, "File results"));
    let status = summary(Some(
        "Choose two files to compare. Text is aligned by line; binary files appear as hexadecimal bytes. Maximum 8 MiB per file.",
    ));
    status.set_xalign(0.0);
    status.set_wrap(true);
    root.append(&status);
    let footer = actions;
    let previous = button("Previous difference");
    let next = button("Next difference");
    previous.set_sensitive(false);
    next.set_sensitive(false);
    footer.append(&previous);
    footer.append(&next);

    let changes = Rc::new(RefCell::new(Vec::<u32>::new()));
    let position = Rc::new(Cell::new(None::<usize>));
    for (button, forward) in [(&next, true), (&previous, false)] {
        let changes = changes.clone();
        let position = position.clone();
        let selection = selection.clone();
        let table = table.clone();
        button.connect_clicked(move |_| {
            let changes = changes.borrow();
            if changes.is_empty() {
                return;
            }
            let index = match position.get() {
                None => {
                    if forward {
                        0
                    } else {
                        changes.len() - 1
                    }
                }
                Some(i) => {
                    if forward {
                        (i + 1) % changes.len()
                    } else {
                        (i + changes.len() - 1) % changes.len()
                    }
                }
            };
            position.set(Some(index));
            selection.set_selected(changes[index]);
            table.scroll_to(
                changes[index],
                None::<&gtk::ColumnViewColumn>,
                gtk::ListScrollFlags::FOCUS,
                None,
            );
        });
    }
    let cancel = Rc::new(RefCell::new(CancelToken::new()));
    let closed = Rc::new(Cell::new(false));
    {
        let cancel = cancel.clone();
        stop.connect_clicked(move |_| cancel.borrow().cancel());
    }
    {
        let cancel = cancel.clone();
        let closed = closed.clone();
        dialog.connect_closed(move |_| {
            closed.set(true);
            cancel.borrow().cancel();
        });
    }
    run.connect_clicked(move |run| {
        let left = VPath::from(left_path.text().as_str());
        let right = VPath::from(right_path.text().as_str());
        for (path, entry) in [(&left, &left_path), (&right, &right_path)] {
            if !path.as_path().is_absolute() {
                entry.add_css_class("error");
                feedback.validate(Some("Enter an absolute file path."), entry);
                return;
            }
            entry.remove_css_class("error");
            feedback.validate(None, entry);
        }
        let token = CancelToken::new();
        *cancel.borrow_mut() = token.clone();
        let work_token = token.clone();
        let vfs = vfs.clone();
        let ignore_case = case.is_active();
        let ignore_space = space.is_active();
        run.set_sensitive(false);
        stop.set_sensitive(true);
        stop.set_visible(true);
        previous.set_sensitive(false);
        next.set_sensitive(false);
        store.remove_all();
        changes.borrow_mut().clear();
        position.set(None);
        status.set_text("Reading files and aligning differences…");
        let run = run.clone();
        let stop = stop.clone();
        let status = status.clone();
        let store = store.clone();
        let closed = closed.clone();
        let feedback = feedback.clone();
        let changes = changes.clone();
        let previous = previous.clone();
        let next = next.clone();
        background(
            token,
            move || {
                crate::features::file_compare::load(
                    vfs.as_ref(),
                    &left,
                    &right,
                    ignore_case,
                    ignore_space,
                    &work_token,
                )
            },
            move |result| {
                if closed.get() {
                    return;
                }
                run.set_sensitive(true);
                stop.set_sensitive(false);
                stop.set_visible(false);
                match result {
                    Ok(result) => {
                        let mut in_change = false;
                        let display = |s: &str| {
                            if result.binary {
                                s.to_owned()
                            } else if s.ends_with('\n') {
                                s.trim_end_matches('\n').replace('\r', " [CR]")
                            } else {
                                format!("{s} [no newline]")
                            }
                        };
                        for (index, row) in result.rows.iter().enumerate() {
                            if row.changed && !in_change {
                                changes.borrow_mut().push(index as u32);
                            }
                            in_change = row.changed;
                            let line = |i: usize| {
                                if result.binary {
                                    format!("{:08x}", i * 16)
                                } else {
                                    (i + 1).to_string()
                                }
                            };
                            let label = if !row.changed {
                                "Same"
                            } else if row.left.is_none() {
                                "Added"
                            } else if row.right.is_none() {
                                "Removed"
                            } else {
                                "Changed"
                            };
                            store.append(&glib::BoxedAnyObject::new(vec![
                                row.left.map(line).unwrap_or_default(),
                                row.left
                                    .map(|i| display(&result.left[i]))
                                    .unwrap_or_default(),
                                row.right.map(line).unwrap_or_default(),
                                row.right
                                    .map(|i| display(&result.right[i]))
                                    .unwrap_or_default(),
                                label.to_owned(),
                            ]));
                        }
                        let count = changes.borrow().len();
                        previous.set_sensitive(count > 0);
                        next.set_sensitive(count > 0);
                        status.set_text(&format!(
                            "{} · Difference groups: {count}{}",
                            if result.binary {
                                "Hexadecimal view · offsets in bytes"
                            } else {
                                "Text comparison"
                            },
                            if count == 0 {
                                " · Files match with the selected options"
                            } else {
                                ""
                            }
                        ));
                    }
                    Err(error) => {
                        status.set_text("No results available. Adjust the options and try again.");
                        feedback.error(&error);
                    }
                }
            },
        );
    });
    dialog.present(Some(&parent));
    run.emit_clicked();
}
