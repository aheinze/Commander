//! Review archive changes before a transactional save.
use super::*;
use crate::archive::edit::{Changes, Snapshot};

pub(super) fn show(source: VPath, input: relm4::Sender<AppMsg>) {
    let Some(parent) = relm4::main_application().active_window() else {
        return;
    };
    let (dialog, view) =
        dialogs::utility_dialog("Edit Archive Contents", 860, 660, "archive-editor-dialog");
    let (root, footer) = power_tools::layout(&view);
    let feedback = notifications::Feedback::default();
    let title = power_tools::summary(Some(&source.to_string()));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    title.set_tooltip_text(Some(&source.to_string()));
    root.append(&title);
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let table = power_tools::text_table(&store, &["Archive path", "Size", "Pending change"]);
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    table.set_model(Some(&selection));
    root.append(&power_tools::table_scroll(&table, "Archive entries"));
    let source_entry = gtk::Entry::builder()
        .placeholder_text("Choose a file to add or replace an entry")
        .build();
    let file_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    source_entry.set_hexpand(true);
    file_row.append(&source_entry);
    let choose = power_tools::button("Choose file…");
    file_row.append(&choose);
    root.append(&power_tools::field("Source file", &file_row));
    source_entry.set_width_chars(1);
    source_entry.update_property(&[gtk::accessible::Property::Label("Source file")]);
    let target = gtk::Entry::builder()
        .placeholder_text("Path inside archive, for example docs/readme.txt")
        .build();
    root.append(&power_tools::field("Archive path", &target));
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let add = power_tools::button("Add or replace");
    let remove = power_tools::button("Remove selected");
    let reset = power_tools::button("Discard changes");
    actions.append(&add);
    actions.append(&remove);
    actions.append(&reset);
    root.append(&actions);
    let status = power_tools::summary(Some("Reading archive…"));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_selectable(true);
    root.append(&status);
    let note = gtk::Label::new(Some(
        "Changes apply only when you save. Removing a folder removes its contents. Saving keeps a recovery copy beside the archive.",
    ));
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.add_css_class("power-tools-note");
    root.append(&note);
    let stop = power_tools::button("Stop");
    let save = power_tools::button("Save archive");
    save.add_css_class("suggested-action");
    let cancel_action = power_tools::cancel_button(&dialog);
    footer.append(&cancel_action);
    footer.append(&stop);
    footer.append(&save);
    for button in [&add, &remove, &reset, &save, &choose] {
        button.set_sensitive(false);
    }
    let snapshot = Rc::new(RefCell::new(None::<Snapshot>));
    let changes = Rc::new(RefCell::new(Changes::default()));
    let names = Rc::new(RefCell::new(Vec::<String>::new()));
    let closed = Rc::new(Cell::new(false));
    let cancel = Rc::new(RefCell::new(CancelToken::new()));
    {
        let closed = closed.clone();
        let cancel = cancel.clone();
        dialog.connect_closed(move |_| {
            closed.set(true);
            cancel.borrow().cancel();
        });
    }
    {
        let cancel = cancel.clone();
        stop.connect_clicked(move |_| cancel.borrow().cancel());
    }
    let redraw: Rc<dyn Fn()> = {
        let snapshot = snapshot.clone();
        let changes = changes.clone();
        let names = names.clone();
        let store = store.clone();
        let status = status.clone();
        let save = save.downgrade();
        Rc::new(move || {
            let borrowed = snapshot.borrow();
            let Some(snapshot) = borrowed.as_ref() else {
                return;
            };
            let changes = changes.borrow();
            store.remove_all();
            names.borrow_mut().clear();
            let mut rows = BTreeMap::new();
            for item in &snapshot.items {
                let removed = changes
                    .removed
                    .iter()
                    .any(|name| item.name == *name || item.name.starts_with(&format!("{name}/")));
                let action = if changes.added.contains_key(&item.name) {
                    "Replace"
                } else if removed {
                    "Remove"
                } else {
                    "Unchanged"
                };
                rows.insert(
                    item.name.clone(),
                    vec![
                        item.name.clone(),
                        if item.directory {
                            "Folder".into()
                        } else {
                            format!("{} B", item.size)
                        },
                        action.into(),
                    ],
                );
            }
            for (name, path) in &changes.added {
                if !rows.contains_key(name) {
                    rows.insert(
                        name.clone(),
                        vec![name.clone(), fs_size(path), "Add".into()],
                    );
                }
            }
            for (name, row) in rows {
                names.borrow_mut().push(name);
                store.append(&glib::BoxedAnyObject::new(row));
            }
            let pending = changes.removed.len() + changes.added.len();
            if let Some(save) = save.upgrade() {
                save.set_sensitive(pending > 0);
            }
            status.set_text(&format!(
                "{} archive entries · {pending} staged changes",
                snapshot.items.len()
            ));
        })
    };
    {
        let names = names.clone();
        let target = target.clone();
        selection.connect_selected_notify(move |selection| {
            if let Some(name) = names.borrow().get(selection.selected() as usize) {
                target.set_text(name);
            }
        });
    }
    {
        let source_entry = source_entry.clone();
        let target = target.clone();
        let parent = parent.downgrade();
        choose.connect_clicked(move |_| {
            let Some(parent) = parent.upgrade() else {
                return;
            };
            let chooser = gtk::FileDialog::builder()
                .title("Add File to Archive")
                .build();
            let source_entry = source_entry.clone();
            let target = target.clone();
            chooser.open(Some(&parent), gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    source_entry.set_text(&path.to_string_lossy());
                    if target.text().is_empty() {
                        target.set_text(&path.file_name().unwrap_or_default().to_string_lossy());
                    }
                }
            });
        });
    }
    {
        let source_entry = source_entry.clone();
        let target = target.clone();
        let changes = changes.clone();
        let feedback = feedback.clone();
        let redraw = redraw.clone();
        add.connect_clicked(move |_| {
            source_entry.remove_css_class("error");
            target.remove_css_class("error");
            let path = std::path::PathBuf::from(source_entry.text().as_str());
            let name = target.text().to_string();
            if !path.is_absolute()
                || !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file())
            {
                source_entry.add_css_class("error");
                feedback.validate(
                    Some("Choose an existing regular file with an absolute path."),
                    &source_entry,
                );
                return;
            }
            if name.is_empty()
                || name.contains(['\\', '\0'])
                || std::path::Path::new(&name)
                    .components()
                    .any(|p| !matches!(p, std::path::Component::Normal(_)))
            {
                target.add_css_class("error");
                feedback.validate(
                    Some("Use a relative archive path without '..' or empty components."),
                    &target,
                );
                return;
            }
            feedback.validate(None, &source_entry);
            feedback.validate(None, &target);
            changes.borrow_mut().added.insert(name.clone(), path);
            changes.borrow_mut().removed.remove(&name);
            redraw();
        });
    }
    {
        let names = names.clone();
        let changes = changes.clone();
        let redraw = redraw.clone();
        remove.connect_clicked(move |_| {
            let selected_name = names.borrow().get(selection.selected() as usize).cloned();
            if let Some(name) = selected_name {
                changes.borrow_mut().added.remove(&name);
                changes.borrow_mut().removed.insert(name);
                redraw();
            }
        });
    }
    {
        let changes = changes.clone();
        let redraw = redraw.clone();
        reset.connect_clicked(move |_| {
            *changes.borrow_mut() = Changes::default();
            redraw();
        });
    }
    {
        let snapshot = snapshot.clone();
        let changes = changes.clone();
        let feedback = feedback.clone();
        let cancel_action = cancel_action.clone();
        let cancel = cancel.clone();
        let closed = closed.clone();
        let status = status.clone();
        let stop = stop.clone();
        let add = add.clone();
        let remove = remove.clone();
        let reset = reset.clone();
        let choose = choose.clone();
        let redraw = redraw.clone();
        let dialog = dialog.downgrade();
        save.connect_clicked(move |save| {
            let Some(snapshot) = snapshot.borrow().clone() else {
                return;
            };
            let changes = changes.borrow().clone();
            for button in [save, &add, &remove, &reset, &choose] {
                button.set_sensitive(false);
            }
            stop.set_sensitive(true);
            stop.set_visible(true);
            status
                .set_text("Writing a new archive. The original remains intact until publication…");
            let Some(dialog) = dialog.upgrade() else {
                return;
            };
            dialog.set_can_close(false);
            let edited_source = snapshot.source.clone();
            let token = CancelToken::new();
            *cancel.borrow_mut() = token.clone();
            let worker_token = token.clone();
            let status = status.clone();
            let feedback = feedback.clone();
            let cancel_action = cancel_action.clone();
            let closed = closed.clone();
            let stop = stop.clone();
            let add = add.clone();
            let remove = remove.clone();
            let reset = reset.clone();
            let choose = choose.clone();
            let input = input.clone();
            let redraw = redraw.clone();
            let dialog = dialog.clone();
            power_tools::background(
                token,
                move || crate::archive::edit::save(&snapshot, &changes, &worker_token),
                move |result| {
                    dialog.set_can_close(true);
                    if closed.get() {
                        return;
                    }
                    stop.set_sensitive(false);
                    stop.set_visible(false);
                    match result {
                        Ok(backup) => {
                            cancel_action.set_label("Close");
                            feedback.success("Archive saved");
                            status.set_text(&format!(
                                "Archive saved. Recovery copy: {}",
                                backup.display()
                            ));
                            let _ = input.send(AppMsg::ArchiveEdited(edited_source.clone()));
                        }
                        Err(error) => {
                            for button in [&add, &remove, &reset, &choose] {
                                button.set_sensitive(true);
                            }
                            redraw();
                            feedback.error(&error);
                        }
                    }
                },
            );
        });
    }
    let token = cancel.borrow().clone();
    let worker_token = token.clone();
    let load_source = source;
    power_tools::background(
        token,
        move || crate::archive::edit::inspect(&load_source, &worker_token),
        move |result| {
            if closed.get() {
                return;
            }
            stop.set_sensitive(false);
            stop.set_visible(false);
            match result {
                Ok(value) => {
                    *snapshot.borrow_mut() = Some(value);
                    for button in [&add, &remove, &reset, &choose] {
                        button.set_sensitive(true);
                    }
                    redraw();
                }
                Err(error) => {
                    status.set_text("Archive unavailable");
                    feedback.error(&error);
                }
            }
        },
    );
    if let Some(parent) = relm4::main_application().active_window() {
        dialog.present(Some(&parent));
    }
}
fn fs_size(path: &std::path::Path) -> String {
    std::fs::metadata(path).map_or_else(|_| "Unavailable".into(), |m| format!("{} B", m.len()))
}
