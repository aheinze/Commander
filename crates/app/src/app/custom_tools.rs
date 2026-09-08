//! A list and focused editor for custom actions. Only saved actions reach the model.

use super::*;

#[cfg(test)]
#[path = "custom_tools/tests.rs"]
mod tests;

const TARGETS: [&str; 3] = ["Files and folders", "Files only", "Folders only"];
const PLACEHOLDERS: [(&str, &str, &str); 4] = [
    (
        "Selected item path",
        "%path%",
        "/home/you/Documents/Report.pdf",
    ),
    (
        "All selected paths",
        "%paths%",
        "One argument for each selected item",
    ),
    ("Item name", "%name%", "Report.pdf"),
    ("Parent folder", "%parent%", "/home/you/Documents"),
];

pub(super) fn show(
    parent: &gtk::Window,
    tools: Vec<CustomToolSession>,
    input: relm4::Sender<AppMsg>,
) {
    let (dialog, manager) = build(tools, input);
    dialog.present(Some(parent));
    manager.add_empty.grab_focus();
}

struct Field {
    root: gtk::Box,
    content: gtk::Box,
    entry: gtk::Entry,
    error: gtk::Label,
}

impl Field {
    fn new(title: &str, placeholder: &str) -> Self {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.set_width_chars(1);
        entry.set_placeholder_text(Some(placeholder));
        entry.update_property(&[gtk::accessible::Property::Label(title)]);
        let error = text("", "tool-field-error");
        error.set_visible(false);
        entry.update_relation(&[gtk::accessible::Relation::DescribedBy(
            &[error.upcast_ref()],
        )]);
        content.append(&entry);
        content.append(&error);
        let root = form_row(title, &content);
        root.add_css_class("tool-form-row");
        if let Some(label) = root.first_child() {
            label.set_valign(gtk::Align::Start);
            label.set_margin_top(8);
        }
        Self {
            root,
            content,
            entry,
            error,
        }
    }

    fn set_error(&self, error: Option<&str>) {
        self.error.set_label(error.unwrap_or_default());
        self.error.set_visible(error.is_some());
        if error.is_some() {
            self.entry.add_css_class("error");
        } else {
            self.entry.remove_css_class("error");
        }
    }
}

struct Manager {
    dialog: glib::WeakRef<adw::Dialog>,
    stack: gtk::Stack,
    list: gtk::ListBox,
    empty: gtk::Box,
    toolbar: gtk::Box,
    count: gtk::Label,
    add_empty: gtk::Button,
    undo_bar: gtk::Box,
    undo_label: gtk::Label,
    tools: RefCell<Vec<CustomToolSession>>,
    removed: RefCell<Option<(usize, CustomToolSession)>>,
    editing: Cell<Option<usize>>,
    original: RefCell<Option<CustomToolSession>>,
    attempted: Cell<bool>,
    loading: Cell<bool>,
    name: Field,
    command: Field,
    extensions: Field,
    target: gtk::DropDown,
    enabled: gtk::CheckButton,
    example: gtk::Label,
    execution: gtk::Label,
    save: gtk::Button,
    input: relm4::Sender<AppMsg>,
}

fn build(
    tools: Vec<CustomToolSession>,
    input: relm4::Sender<AppMsg>,
) -> (adw::Dialog, Rc<Manager>) {
    let (dialog, view) = utility_dialog("Context Menu Tools", 600, 540, "tools-dialog");
    let stack = gtk::Stack::new();
    stack.set_hhomogeneous(false);
    stack.set_vhomogeneous(false);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(90);
    view.set_content(Some(&stack));

    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.add_css_class("dialog-body");
    body.set_vexpand(true);
    body.append(&text(
        "Manage the custom commands shown in the file context menu.",
        "dialog-hint",
    ));
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.set_margin_top(4);
    let count = text("", "tool-caption");
    count.set_hexpand(true);
    let add = gtk::Button::with_label("Add action");
    add.add_css_class("tool-button");
    add.add_css_class("suggested-action");
    toolbar.append(&count);
    toolbar.append(&add);
    body.append(&toolbar);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("tool-list");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .overlay_scrolling(false)
        .vexpand(true)
        .child(&list)
        .build();
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 8);
    empty.add_css_class("tool-empty");
    empty.set_valign(gtk::Align::Center);
    empty.set_vexpand(true);
    let icon = gtk::Image::from_icon_name("commander-app-window-symbolic");
    icon.set_pixel_size(28);
    icon.add_css_class("tool-empty-icon");
    empty.append(&icon);
    let title = text("No custom actions", "tool-row-title");
    title.set_xalign(0.5);
    empty.append(&title);
    let explanation = text(
        "Add a command, then choose which files or folders it applies to.",
        "tool-caption",
    );
    explanation.set_xalign(0.5);
    explanation.set_justify(gtk::Justification::Center);
    explanation.set_max_width_chars(42);
    empty.append(&explanation);
    let add_empty = gtk::Button::with_label("Add action");
    add_empty.add_css_class("tool-button");
    add_empty.add_css_class("suggested-action");
    add_empty.set_halign(gtk::Align::Center);
    add_empty.set_margin_top(8);
    empty.append(&add_empty);
    // An overlay gives the empty state the same flexible space as the list.
    let content = gtk::Overlay::new();
    content.set_child(Some(&scroll));
    content.add_overlay(&empty);
    content.set_vexpand(true);
    body.append(&content);
    let undo_bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    undo_bar.add_css_class("tool-undo");
    let undo_label = text("", "tool-caption");
    undo_label.set_hexpand(true);
    let undo = gtk::Button::with_label("Undo");
    undo.add_css_class("settings-link");
    undo_bar.append(&undo_label);
    undo_bar.append(&undo);
    undo_bar.set_visible(false);
    body.append(&undo_bar);
    page.append(&body);
    let footer = dialog_actions();
    let saved = text("Changes save automatically", "dialog-hint");
    footer.prepend(&saved);
    let done = gtk::Button::with_label("Done");
    footer.append(&done);
    page.append(&footer);
    stack.add_named(&page, Some("list"));

    let editor = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let form = gtk::Box::new(gtk::Orientation::Vertical, 12);
    form.add_css_class("dialog-body");
    let name = Field::new("Name", "e.g. Open in image editor");
    form.append(&name.root);
    let command = Field::new("Command", "e.g. gimp %paths%");
    command.entry.add_css_class("monospace");
    form.append(&command.root);
    let insert = gtk::MenuButton::builder()
        .label("Insert placeholder")
        .build();
    insert.add_css_class("tool-placeholder-menu");
    insert.set_halign(gtk::Align::Start);
    let placeholders = gtk::Popover::new();
    let choices = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let mut placeholder_buttons = Vec::new();
    for (label, token, example) in PLACEHOLDERS {
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("open-with-row");
        let row = gtk::Box::new(gtk::Orientation::Vertical, 3);
        row.append(&text(label, "tool-field-label"));
        row.append(&text(&format!("{token} · {example}"), "tool-caption"));
        button.set_child(Some(&row));
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        choices.append(&button);
        placeholder_buttons.push((button, token));
    }
    placeholders.set_child(Some(&choices));
    insert.set_popover(Some(&placeholders));
    command.content.append(&insert);
    let example = text("", "tool-command-example");
    example.set_selectable(true);
    example.set_can_focus(false);
    let execution = text("", "tool-caption");
    command.content.append(&execution);
    let preview = gtk::Expander::builder()
        .label("Preview with example items")
        .child(&example)
        .build();
    command.content.append(&preview);

    let target = gtk::DropDown::from_strings(&TARGETS);
    target.update_property(&[gtk::accessible::Property::Label("Show for")]);
    let target_row = form_row("Show for", &target);
    target_row.add_css_class("tool-form-row");
    form.append(&target_row);
    let extensions = Field::new("File types", "All file types");
    extensions.content.append(&text(
        "Optional. Separate extensions with commas, e.g. png, jpg.",
        "tool-caption",
    ));
    form.append(&extensions.root);
    let enabled = gtk::CheckButton::with_label("Show this action in the context menu");
    form.append(&form_row("", &enabled));
    let tips = text(
        "Use an app name or executable path, followed by its arguments. Quote paths containing spaces. Commands run directly; use sh -c if you need shell features such as pipes or redirects.",
        "tool-caption",
    );
    let help = gtk::Expander::builder()
        .label("Command tips")
        .child(&tips)
        .build();
    form.append(&form_row("", &help));
    let form_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .overlay_scrolling(false)
        .vexpand(true)
        .child(&form)
        .build();
    editor.append(&form_scroll);
    let actions = dialog_actions();
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Add action");
    save.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&save);
    editor.append(&actions);
    stack.add_named(&editor, Some("editor"));

    let manager = Rc::new(Manager {
        dialog: dialog.downgrade(),
        stack,
        list,
        empty,
        toolbar,
        count,
        add_empty,
        undo_bar,
        undo_label,
        tools: RefCell::new(tools),
        removed: RefCell::new(None),
        editing: Cell::new(None),
        original: RefCell::new(None),
        attempted: Cell::new(false),
        loading: Cell::new(false),
        name,
        command,
        extensions,
        target,
        enabled,
        example,
        execution,
        save,
        input,
    });
    for button in [&add, &manager.add_empty] {
        let weak = Rc::downgrade(&manager);
        button.connect_clicked(move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.edit(None);
            }
        });
    }
    let weak = dialog.downgrade();
    done.connect_clicked(move |_| {
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });
    let weak = Rc::downgrade(&manager);
    cancel.connect_clicked(move |_| {
        if let Some(manager) = weak.upgrade() {
            manager.leave_editor(false);
        }
    });
    let weak = Rc::downgrade(&manager);
    manager.save.connect_clicked(move |_| {
        if let Some(manager) = weak.upgrade() {
            manager.save_action();
        }
    });
    let weak = Rc::downgrade(&manager);
    undo.connect_clicked(move |_| {
        if let Some(manager) = weak.upgrade() {
            let removed = manager.removed.borrow_mut().take();
            if let Some((index, tool)) = removed {
                let index = index.min(manager.tools.borrow().len());
                manager.tools.borrow_mut().insert(index, tool);
                manager.publish();
                manager.render();
                manager.undo_bar.set_visible(false);
            }
        }
    });
    for entry in [
        &manager.name.entry,
        &manager.command.entry,
        &manager.extensions.entry,
    ] {
        let weak = Rc::downgrade(&manager);
        entry.connect_changed(move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.update_form();
            }
        });
        let weak = Rc::downgrade(&manager);
        entry.connect_activate(move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.save_action();
            }
        });
    }
    let weak = Rc::downgrade(&manager);
    manager.target.connect_selected_notify(move |_| {
        if let Some(manager) = weak.upgrade() {
            manager.update_form();
        }
    });
    for (button, token) in placeholder_buttons {
        let weak = Rc::downgrade(&manager);
        let popover = placeholders.downgrade();
        button.connect_clicked(move |_| {
            if let Some(manager) = weak.upgrade() {
                insert_placeholder(&manager.command.entry, token);
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
                manager.command.entry.grab_focus();
            }
        });
    }
    let weak = Rc::downgrade(&manager);
    dialog.connect_close_attempt(move |_| {
        if let Some(manager) = weak.upgrade() {
            manager.leave_editor(true);
        }
    });
    // The dialog owns the controller only while it is open; widget callbacks are weak.
    let lifetime = RefCell::new(Some(Rc::clone(&manager)));
    dialog.connect_closed(move |_| {
        lifetime.borrow_mut().take();
    });
    manager.render();
    (dialog, manager)
}

impl Manager {
    fn render(self: &Rc<Self>) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let tools = self.tools.borrow();
        self.empty.set_visible(tools.is_empty());
        self.toolbar.set_visible(!tools.is_empty());
        self.list.set_visible(!tools.is_empty());
        self.count.set_label(&format!(
            "{} {} · {} enabled",
            tools.len(),
            if tools.len() == 1 {
                "action"
            } else {
                "actions"
            },
            tools.iter().filter(|tool| tool.enabled).count()
        ));
        for (index, tool) in tools.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("tool-row");
            let details = gtk::Box::new(gtk::Orientation::Vertical, 3);
            details.set_hexpand(true);
            let title = text(&tool.name, "tool-row-title");
            title.set_wrap(false);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.set_tooltip_text(Some(&tool.name));
            details.append(&title);
            let scope = match tool.applies_to.as_str() {
                "files" => "Files",
                "folders" => "Folders",
                _ => "Files and folders",
            };
            let scope = if tool.applies_to != "folders" && !tool.extensions.is_empty() {
                format!("{scope} · {}", tool.extensions)
            } else {
                scope.to_owned()
            };
            let subtitle = text(&scope, "tool-caption");
            subtitle.set_wrap(false);
            subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
            details.append(&subtitle);
            let command = text(&tool.command, "tool-row-command");
            command.set_wrap(false);
            command.set_ellipsize(gtk::pango::EllipsizeMode::End);
            command.set_tooltip_text(Some(&tool.command));
            details.append(&command);
            row.append(&details);
            let enabled = gtk::Switch::new();
            enabled.set_valign(gtk::Align::Center);
            enabled.set_active(tool.enabled);
            enabled.update_property(&[gtk::accessible::Property::Label(&format!(
                "Enable {}",
                tool.name
            ))]);
            enabled.set_tooltip_text(Some("Show in context menu"));
            let weak = Rc::downgrade(self);
            enabled.connect_active_notify(move |enabled| {
                if let Some(manager) = weak.upgrade() {
                    manager.tools.borrow_mut()[index].enabled = enabled.is_active();
                    manager.publish();
                    let tools = manager.tools.borrow();
                    manager.count.set_label(&format!(
                        "{} {} · {} enabled",
                        tools.len(),
                        if tools.len() == 1 {
                            "action"
                        } else {
                            "actions"
                        },
                        tools.iter().filter(|tool| tool.enabled).count()
                    ));
                }
            });
            row.append(&enabled);
            let edit = icon_button(
                "commander-file-pen-line-symbolic",
                &format!("Edit {}", tool.name),
            );
            edit.add_css_class("flat");
            edit.set_valign(gtk::Align::Center);
            let weak = Rc::downgrade(self);
            edit.connect_clicked(move |_| {
                if let Some(manager) = weak.upgrade() {
                    manager.edit(Some(index));
                }
            });
            row.append(&edit);
            let remove = icon_button("commander-trash-symbolic", &format!("Remove {}", tool.name));
            remove.add_css_class("flat");
            remove.set_valign(gtk::Align::Center);
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(manager) = weak.upgrade() {
                    let tool = manager.tools.borrow_mut().remove(index);
                    manager
                        .undo_label
                        .set_label(&format!("Removed “{}”", tool.name));
                    *manager.removed.borrow_mut() = Some((index, tool));
                    manager.undo_bar.set_visible(true);
                    manager.publish();
                    manager.render();
                }
            });
            row.append(&remove);
            self.list.append(&row);
        }
    }

    fn edit(&self, index: Option<usize>) {
        let tool = index
            .and_then(|index| self.tools.borrow().get(index).cloned())
            .unwrap_or_else(blank_tool);
        self.loading.set(true);
        self.editing.set(index);
        self.name.entry.set_text(&tool.name);
        self.command.entry.set_text(&tool.command);
        self.target.set_selected(match tool.applies_to.as_str() {
            "files" => 1,
            "folders" => 2,
            _ => 0,
        });
        self.extensions.entry.set_text(&tool.extensions);
        self.enabled.set_active(tool.enabled);
        *self.original.borrow_mut() = Some(tool);
        self.attempted.set(false);
        self.loading.set(false);
        self.update_form();
        self.save.set_label(if index.is_some() {
            "Save action"
        } else {
            "Add action"
        });
        self.stack.set_visible_child_name("editor");
        if let Some(dialog) = self.dialog.upgrade() {
            dialog.set_title(if index.is_some() {
                "Edit Custom Action"
            } else {
                "Add Custom Action"
            });
            dialog.set_can_close(false);
        }
        self.name.entry.grab_focus();
    }

    fn draft(&self) -> CustomToolSession {
        CustomToolSession {
            name: self.name.entry.text().to_string(),
            command: self.command.entry.text().to_string(),
            applies_to: match self.target.selected() {
                1 => "files",
                2 => "folders",
                _ => "both",
            }
            .to_owned(),
            extensions: self.extensions.entry.text().to_string(),
            enabled: self.enabled.is_active(),
        }
    }

    fn update_form(&self) {
        if self.loading.get() {
            return;
        }
        let tool = self.draft();
        self.extensions
            .root
            .set_visible(tool.applies_to != "folders");
        let errors = validate(&tool);
        for (field, error) in [
            (&self.name, errors[0]),
            (&self.command, errors[1]),
            (&self.extensions, errors[2]),
        ] {
            field.set_error(if self.attempted.get() { error } else { None });
        }
        let tokens = shlex::split(&tool.command).unwrap_or_default();
        self.execution
            .set_label(if tokens.iter().any(|token| token == "%paths%") {
                "Runs once with all selected items."
            } else {
                "Runs once for each selected item."
            });
        self.example.set_label(
            &command_example(&tool.command, tool.applies_to == "folders")
                .unwrap_or_else(|| "Enter a valid command to see an example.".to_owned()),
        );
    }

    fn save_action(self: &Rc<Self>) {
        self.attempted.set(true);
        self.update_form();
        let mut tool = self.draft();
        let errors = validate(&tool);
        if let Some(index) = errors.iter().position(Option::is_some) {
            [&self.name, &self.command, &self.extensions][index]
                .entry
                .grab_focus();
            return;
        }
        tool.name = tool.name.trim().to_owned();
        tool.command = tool.command.trim().to_owned();
        tool.extensions = if tool.applies_to == "folders" {
            String::new()
        } else {
            extensions(&tool.extensions).unwrap()
        };
        if let Some(index) = self.editing.get() {
            self.tools.borrow_mut()[index] = tool;
        } else {
            self.tools.borrow_mut().push(tool);
        }
        self.publish();
        self.render();
        self.show_list();
    }

    fn publish(&self) {
        let _ = self
            .input
            .send(AppMsg::SetCustomTools(self.tools.borrow().clone()));
    }

    fn show_list(&self) {
        self.stack.set_visible_child_name("list");
        if let Some(dialog) = self.dialog.upgrade() {
            dialog.set_can_close(true);
            dialog.set_title("Context Menu Tools");
        }
        if let Some(row) = self.list.last_child() {
            row.child_focus(gtk::DirectionType::TabForward);
        }
    }

    fn leave_editor(self: &Rc<Self>, close: bool) {
        if self.original.borrow().as_ref() == Some(&self.draft()) {
            if close {
                if let Some(dialog) = self.dialog.upgrade() {
                    dialog.force_close();
                }
            } else {
                self.show_list();
            }
            return;
        }
        let Some(parent) = self.dialog.upgrade() else {
            return;
        };
        let confirm = AlertSheet::new(
            Some("Discard changes?"),
            Some("This action has unsaved changes."),
        );
        confirm.add_response("keep", "Keep editing");
        confirm.add_response("discard", "Discard changes");
        confirm.set_close_response("keep");
        confirm.set_default_response(Some("keep"));
        let weak = Rc::downgrade(self);
        confirm.connect_response(Some("discard"), move |_, _| {
            if let Some(manager) = weak.upgrade() {
                if close {
                    if let Some(dialog) = manager.dialog.upgrade() {
                        dialog.force_close();
                    }
                } else {
                    manager.show_list();
                }
            }
        });
        confirm.present(Some(&parent));
    }
}

fn blank_tool() -> CustomToolSession {
    CustomToolSession {
        name: String::new(),
        command: String::new(),
        applies_to: "both".to_owned(),
        extensions: String::new(),
        enabled: true,
    }
}

fn text(label: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.add_css_class(class);
    label
}

fn extensions(value: &str) -> Result<String, &'static str> {
    let mut extensions = Vec::new();
    for value in value
        .split([',', ' ', ';'])
        .filter(|value| !value.is_empty())
    {
        let value = value
            .trim_start_matches("*.")
            .trim_start_matches('.')
            .to_lowercase();
        if value.is_empty()
            || !value
                .chars()
                .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-')
        {
            return Err("Use extensions such as pdf, png or rs. For .tar.gz files, use gz.");
        }
        if !extensions.contains(&value) {
            extensions.push(value);
        }
    }
    Ok(extensions.join(", "))
}

fn validate(tool: &CustomToolSession) -> [Option<&'static str>; 3] {
    let name = tool
        .name
        .trim()
        .is_empty()
        .then_some("Give this action a name so you can find it in the menu.");
    let command = if tool.command.trim().is_empty() {
        Some("Enter the app or command to run.")
    } else if tool.command.contains('\0') {
        Some("Remove the invalid character from this command.")
    } else {
        match shlex::split(&tool.command) {
            None => Some("A quote is missing its closing partner. Check the command’s quotes."),
            Some(tokens) if tokens.first().is_none_or(String::is_empty) => {
                Some("Start with an app name or executable path.")
            }
            _ => None,
        }
    };
    let extensions = if tool.applies_to == "folders" {
        None
    } else {
        extensions(&tool.extensions).err()
    };
    [name, command, extensions]
}

fn command_example(command: &str, folders: bool) -> Option<String> {
    let tokens = shlex::split(command)?;
    if tokens.is_empty() {
        return None;
    }
    let mut expanded = Vec::new();
    let names = if folders {
        ["Project", "Photos"]
    } else {
        ["Report.pdf", "Notes.pdf"]
    };
    let paths = names.map(|name| format!("/home/you/Documents/{name}"));
    for token in tokens {
        if token == "%paths%" {
            expanded.extend(paths.iter().cloned());
        } else {
            expanded.push(
                token
                    .replace("%path%", &paths[0])
                    .replace("%name%", names[0])
                    .replace("%parent%", "/home/you/Documents")
                    .replace("%paths%", &paths[0]),
            );
        }
    }
    shlex::try_join(expanded.iter().map(String::as_str)).ok()
}

fn insert_placeholder(entry: &gtk::Entry, token: &str) {
    let mut position = entry.position();
    if let Some((start, _)) = entry.selection_bounds() {
        position = start;
        entry.delete_selection();
    }
    // At the end of an existing argument, separate the new placeholder automatically.
    let before = entry.text().chars().take(position as usize).last();
    let after = entry.text().chars().nth(position as usize);
    let prefix = if before.is_some_and(|ch| !ch.is_whitespace() && ch != '"' && ch != '\'') {
        " "
    } else {
        ""
    };
    let suffix = if after.is_some_and(|ch| !ch.is_whitespace() && ch != '"' && ch != '\'') {
        " "
    } else {
        ""
    };
    entry.insert_text(&format!("{prefix}{token}{suffix}"), &mut position);
    entry.set_position(position);
}
