//! Live rename plans use the same names for preview and execution.
use super::*;

pub(super) fn show(sources: Vec<VPath>, sender: &ComponentSender<AppModel>) {
    let Some(parent) = relm4::main_application().active_window() else {
        return;
    };
    let (dialog, view) = dialogs::utility_dialog("Batch Rename", 760, 620, "batch-rename-dialog");
    let (root, actions) = power_tools::layout(&view);
    let feedback = notifications::Feedback::default();
    let method = gtk::DropDown::from_strings(&["Replace", "Add Text", "Number", "Change Case"]);
    root.append(&power_tools::field("Method", &method));
    let find = gtk::Entry::builder()
        .placeholder_text("Text to find")
        .build();
    let replacement = gtk::Entry::builder()
        .placeholder_text("Replacement; regex groups use $1 or ${name}")
        .build();
    let match_case = gtk::CheckButton::with_label("Match case");
    let regex = gtk::CheckButton::with_label("Regular expression");
    let replace_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    replace_box.append(&power_tools::field("Find", &find));
    replace_box.append(&power_tools::field("Replace", &replacement));
    let toggles = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    toggles.append(&match_case);
    toggles.append(&regex);
    replace_box.append(&toggles);
    let prefix = gtk::Entry::new();
    let suffix = gtk::Entry::new();
    let add_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    add_box.append(&power_tools::field("Prefix", &prefix));
    add_box.append(&power_tools::field("Suffix", &suffix));
    let template = gtk::Entry::new();
    template.set_text("{name} {n}");
    template.set_tooltip_text(Some("{name}: original name; {n}: sequence number"));
    let start = gtk::SpinButton::with_range(0.0, 999_999.0, 1.0);
    start.set_value(1.0);
    let padding = gtk::SpinButton::with_range(1.0, 8.0, 1.0);
    padding.set_value(2.0);
    let number_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    number_box.append(&power_tools::field("Template", &template));
    let numbers = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    numbers.append(&power_tools::field("Start", &start));
    numbers.append(&power_tools::field("Digits", &padding));
    number_box.append(&numbers);
    let case_mode = gtk::DropDown::from_strings(&["lowercase", "UPPERCASE", "Title Case"]);
    let case_box = power_tools::field("Case", &case_mode);
    for section in [&replace_box, &add_box, &number_box, &case_box] {
        root.append(section);
    }
    let keep = gtk::CheckButton::with_label("Keep file extensions unchanged");
    keep.set_active(true);
    root.append(&keep);
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let table = power_tools::text_table(&store, &["Current name", "New name", "Status"]);
    root.append(&power_tools::table_scroll(&table, "Rename preview"));
    let status = power_tools::summary(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    root.append(&status);
    let apply = power_tools::button("Rename items");
    apply.add_css_class("suggested-action");
    dialog.set_default_widget(Some(&apply));
    actions.append(&power_tools::cancel_button(&dialog));
    actions.append(&apply);
    let planned = Rc::new(RefCell::new(Vec::<(VPath, String)>::new()));
    let refresh: Rc<dyn Fn()> = {
        let method = method.clone();
        let find = find.clone();
        let replacement = replacement.clone();
        let prefix = prefix.clone();
        let suffix = suffix.clone();
        let template = template.clone();
        let start = start.clone();
        let padding = padding.clone();
        let case_mode = case_mode.clone();
        let keep = keep.clone();
        let regex = regex.clone();
        let match_case = match_case.clone();
        let apply = apply.clone();
        let planned = planned.clone();
        Rc::new(move || {
            for (index, section) in [&replace_box, &add_box, &number_box, &case_box]
                .iter()
                .enumerate()
            {
                section.set_visible(method.selected() as usize == index);
            }
            feedback.validate(None, &find);
            find.remove_css_class("error");
            store.remove_all();
            planned.borrow_mut().clear();
            let expression = if method.selected() == 0 && regex.is_active() {
                match regex::RegexBuilder::new(&find.text())
                    .case_insensitive(!match_case.is_active())
                    .build()
                {
                    Ok(value) => Some(value),
                    Err(error) => {
                        status.set_text("Preview unavailable");
                        find.add_css_class("error");
                        feedback
                            .validate(Some(&format!("Invalid regular expression: {error}")), &find);
                        apply.set_sensitive(false);
                        return;
                    }
                }
            } else {
                None
            };
            let items: Vec<_> = sources
                .iter()
                .enumerate()
                .map(|(index, source)| {
                    let original = source
                        .file_name()
                        .and_then(OsStr::to_str)
                        .unwrap_or_default();
                    let (stem, extension) =
                        split_extension(original, keep.is_active() && !source.as_path().is_dir());
                    let next = match method.selected() {
                        0 => expression.as_ref().map_or_else(
                            || {
                                replace_text(
                                    &stem,
                                    &find.text(),
                                    &replacement.text(),
                                    match_case.is_active(),
                                )
                            },
                            |re| {
                                re.replace_all(&stem, replacement.text().as_str())
                                    .into_owned()
                            },
                        ),
                        1 => format!("{}{}{}", prefix.text(), stem, suffix.text()),
                        2 => {
                            let n = start.value_as_int() as usize + index;
                            let width = padding.value_as_int() as usize;
                            template
                                .text()
                                .replace("{name}", &stem)
                                .replace("{n}", &format!("{n:0width$}"))
                        }
                        3 => match case_mode.selected() {
                            1 => stem.to_uppercase(),
                            2 => title_case(&stem),
                            _ => stem.to_lowercase(),
                        },
                        _ => stem,
                    };
                    (
                        source.clone(),
                        format!("{next}{extension}").trim().to_owned(),
                    )
                })
                .collect();
            let errors = crate::features::rename_preview::validate(&items);
            let mut changed = 0;
            for ((source, name), error) in items.iter().zip(&errors) {
                let original = source
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let differs = original != *name;
                if differs {
                    changed += 1;
                }
                let label = error
                    .clone()
                    .unwrap_or_else(|| if differs { "Ready" } else { "Unchanged" }.to_owned());
                store.append(&glib::BoxedAnyObject::new(vec![
                    original,
                    name.clone(),
                    label,
                ]));
            }
            let problems = errors.iter().filter(|e| e.is_some()).count();
            status.set_text(&format!("Items: {} · Changes: {changed} · Conflicts: {problems}. Review the new names before renaming.", items.len()));
            apply.set_sensitive(changed > 0 && problems == 0);
            *planned.borrow_mut() = items;
        })
    };
    let mut signals = Vec::new();
    for entry in [&find, &replacement, &prefix, &suffix, &template] {
        let refresh = refresh.clone();
        let id = entry.connect_changed(move |_| refresh());
        signals.push((entry.clone().upcast::<glib::Object>(), id));
    }
    for toggle in [&match_case, &regex, &keep] {
        let refresh = refresh.clone();
        let id = toggle.connect_toggled(move |_| refresh());
        signals.push((toggle.clone().upcast::<glib::Object>(), id));
    }
    for dropdown in [&method, &case_mode] {
        let refresh = refresh.clone();
        let id = dropdown.connect_selected_notify(move |_| refresh());
        signals.push((dropdown.clone().upcast::<glib::Object>(), id));
    }
    for spin in [&start, &padding] {
        let refresh = refresh.clone();
        let id = spin.connect_value_changed(move |_| refresh());
        signals.push((spin.clone().upcast::<glib::Object>(), id));
    }
    let input = sender.input_sender().clone();
    let close = dialog.downgrade();
    apply.connect_clicked(move |_| {
        let _ = input.send(AppMsg::BatchRename(planned.borrow().clone()));
        if let Some(close) = close.upgrade() {
            close.close();
        }
    });
    let signals = RefCell::new(signals);
    dialog.connect_closed(move |_| {
        for (object, id) in signals.take() {
            object.disconnect(id);
        }
    });
    refresh();
    dialog.present(Some(&parent));
    find.grab_focus();
}
