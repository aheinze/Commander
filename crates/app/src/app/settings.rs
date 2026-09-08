//! Searchable keyboard configuration and staged application preferences.

use super::dialogs::{dialog_actions, utility_dialog};
use super::*;

#[cfg(test)]
mod tests;
mod updates;

#[derive(Clone, Debug)]
pub(crate) struct SettingsDraft {
    pub appearance: AppearanceMode,
    pub color_theme: ColorTheme,
    pub parallel_transfers: bool,
    pub workflow: WorkflowPreferences,
    pub profile: KeymapProfile,
    pub overrides: KeymapOverrides,
    pub clear_recent: bool,
}

impl SettingsDraft {
    fn from_model(model: &AppModel) -> Self {
        Self {
            appearance: model.appearance,
            color_theme: model.color_theme,
            parallel_transfers: model.parallel_transfers,
            workflow: model.workflow.clone(),
            profile: model.keymap.profile(),
            overrides: model.keymap.overrides(),
            clear_recent: false,
        }
    }

    fn keymap(&self) -> Keymap {
        Keymap::new(self.profile, self.overrides.clone())
    }
}

pub(super) fn startup_pane(
    saved: &PaneSession,
    restore: bool,
    path: Option<&VPath>,
) -> PaneSession {
    let mut pane = saved.clone();
    if !restore {
        pane.tabs.clear();
        pane.active_tab = 0;
        pane.locations.clear();
        pane.folders.clear();
    }
    overridden_session(&pane, path)
}

pub(super) fn show(model: &AppModel, sender: &ComponentSender<AppModel>) {
    if let Some(parent) = relm4::main_application().active_window() {
        let (dialog, _) = build(
            SettingsDraft::from_model(model),
            sender.input_sender().clone(),
        );
        dialog.present(Some(&parent));
    }
}

struct SettingsUi {
    draft: Rc<RefCell<SettingsDraft>>,
    stack: gtk::Stack,
    search: gtk::SearchEntry,
    shortcuts: gtk::ListBox,
    apply: gtk::Button,
    cancel: gtk::Button,
}

fn build(draft: SettingsDraft, input: relm4::Sender<AppMsg>) -> (adw::Dialog, Rc<SettingsUi>) {
    let initial = draft.clone();
    let draft = Rc::new(RefCell::new(draft));
    let (dialog, view) = utility_dialog("Settings", 780, 680, "settings-dialog");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.set_vexpand(true);
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_hhomogeneous(false);
    stack.set_vhomogeneous(false);
    let navigation = gtk::StackSidebar::new();
    navigation.set_stack(&stack);
    navigation.add_css_class("settings-navigation");
    navigation.set_size_request(145, -1);
    body.append(&navigation);
    body.append(&stack);
    root.append(&body);

    let workflow = page(
        &stack,
        "workflow",
        "Workflow",
        "Make Commander fit the way you work.",
    );
    section(&workflow, "Startup & navigation");
    toggle(
        &workflow,
        "Restore tabs and folders",
        "Reopen your previous locations on startup. Otherwise, start in Home.",
        initial.workflow.restore_tabs,
        {
            let draft = draft.clone();
            move |value| draft.borrow_mut().workflow.restore_tabs = value
        },
    );
    toggle(
        &workflow,
        "Keep folders first",
        "Group folders before files in both panes, regardless of sort direction.",
        initial.workflow.directories_first,
        {
            let draft = draft.clone();
            move |value| draft.borrow_mut().workflow.directories_first = value
        },
    );
    toggle(
        &workflow,
        "Browse archives in Commander",
        "Open supported archives as folders. Turn off to use the default application.",
        initial.workflow.browse_archives,
        {
            let draft = draft.clone();
            move |value| draft.borrow_mut().workflow.browse_archives = value
        },
    );
    section(&workflow, "File transfers");
    toggle(
        &workflow,
        "Parallel transfers",
        "Copy file contents concurrently when the storage supports it. Turn off for sequential transfers.",
        initial.parallel_transfers,
        {
            let draft = draft.clone();
            move |value| draft.borrow_mut().parallel_transfers = value
        },
    );
    note(
        &workflow,
        "Copied files are verified automatically. Undo and recovery stay available.",
    );
    section(&workflow, "Custom actions");
    let tools = action(
        &workflow,
        "Context menu tools",
        "Manage commands for your selected files and folders.",
        "Manage…",
    );
    tools.connect_clicked({
        let input = input.clone();
        move |_| {
            let _ = input.send(AppMsg::ManageCustomTools);
        }
    });

    let keyboard = page(
        &stack,
        "keyboard",
        "Keyboard",
        "Find an action, then press the shortcut you want to use in the file panes.",
    );
    let profile =
        gtk::DropDown::from_strings(&["Classic · function keys", "Modern · familiar shortcuts"]);
    profile.set_selected(u32::from(initial.profile == KeymapProfile::Modern));
    row(
        &keyboard,
        "Keyboard profile",
        "Custom shortcuts are saved separately for each profile.",
        &profile,
    );
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search actions or shortcuts…"));
    search.update_property(&[gtk::accessible::Property::Label(
        "Search keyboard shortcuts",
    )]);
    search.add_css_class("settings-search");
    keyboard.append(&search);
    let shortcuts = gtk::ListBox::new();
    shortcuts.set_selection_mode(gtk::SelectionMode::None);
    shortcuts.add_css_class("settings-shortcuts");
    let empty = gtk::Label::new(Some("No matching actions. Try another search."));
    empty.set_margin_top(24);
    empty.set_margin_bottom(24);
    empty.set_wrap(true);
    shortcuts.set_placeholder(Some(&empty));
    keyboard.append(&shortcuts);
    let reset = gtk::Button::with_label("Reset this profile to defaults");
    reset.set_halign(gtk::Align::Start);
    reset.set_margin_top(14);
    keyboard.append(&reset);

    let appearance = page(
        &stack,
        "appearance",
        "Appearance",
        "Choose a comfortable palette for your workspace.",
    );
    let mode = gtk::DropDown::from_strings(&["Follow system", "Light", "Dark"]);
    mode.set_selected(match initial.appearance {
        AppearanceMode::System => 0,
        AppearanceMode::Light => 1,
        AppearanceMode::Dark => 2,
    });
    row(
        &appearance,
        "Appearance",
        "Follow the desktop or choose a fixed appearance.",
        &mode,
    );
    mode.connect_selected_notify({
        let draft = draft.clone();
        move |picker| {
            draft.borrow_mut().appearance = match picker.selected() {
                0 => AppearanceMode::System,
                1 => AppearanceMode::Light,
                _ => AppearanceMode::Dark,
            }
        }
    });
    let theme = gtk::DropDown::from_strings(&[
        "Automatic (Omarchy)",
        "Carelo Graphite",
        "Midnight Blue",
        "Forest",
        "Aubergine",
    ]);
    theme.set_selected(match initial.color_theme {
        ColorTheme::Automatic => 0,
        ColorTheme::Carelo => 1,
        ColorTheme::Midnight => 2,
        ColorTheme::Forest => 3,
        ColorTheme::Aubergine => 4,
    });
    row(
        &appearance,
        "Color theme",
        "Automatic follows Omarchy and falls back to Carelo Graphite.",
        &theme,
    );
    theme.connect_selected_notify({
        let draft = draft.clone();
        move |picker| {
            draft.borrow_mut().color_theme = match picker.selected() {
                1 => ColorTheme::Carelo,
                2 => ColorTheme::Midnight,
                3 => ColorTheme::Forest,
                4 => ColorTheme::Aubergine,
                _ => ColorTheme::Automatic,
            }
        }
    });
    if let Some(name) = omarchy::current_theme_name() {
        note(
            &appearance,
            &format!("Automatic is following the {name} palette."),
        );
    }
    section(&appearance, "Inspector");
    toggle(
        &appearance,
        "Calculate folder sizes",
        "Scan folder contents for the inspector. Turn off to reduce background work on large or remote folders.",
        initial.workflow.inspector_folder_sizes,
        {
            let draft = draft.clone();
            move |value| draft.borrow_mut().workflow.inspector_folder_sizes = value
        },
    );
    toggle(
        &appearance,
        "Show Git information",
        "Look up repository status for the focused item in the inspector.",
        initial.workflow.inspector_git,
        {
            let draft = draft.clone();
            move |value| draft.borrow_mut().workflow.inspector_git = value
        },
    );

    let privacy = page(
        &stack,
        "privacy",
        "History",
        "Control the recent locations shown in the sidebar and command palette.",
    );
    let remember = toggle(
        &privacy,
        "Remember recent locations",
        "Turning this off also clears the existing recent-location list when you apply.",
        initial.workflow.remember_recent,
        {
            let draft = draft.clone();
            move |value| draft.borrow_mut().workflow.remember_recent = value
        },
    );
    let limit = gtk::SpinButton::with_range(5.0, 100.0, 1.0);
    limit.set_value(f64::from(initial.workflow.recent_limit.clamp(5, 100)));
    limit.set_sensitive(initial.workflow.remember_recent);
    row(
        &privacy,
        "Recent-location limit",
        "Keep between 5 and 100 locations.",
        &limit,
    );
    limit.connect_value_changed({
        let draft = draft.clone();
        move |spin| draft.borrow_mut().workflow.recent_limit = spin.value_as_int() as u32
    });
    remember.connect_active_notify({
        let limit = limit.clone();
        move |switch| limit.set_sensitive(switch.is_active())
    });
    let clear = action(
        &privacy,
        "Clear recent locations",
        "Bookmarks, saved workspaces, and undo history are kept.",
        "Clear…",
    );
    clear.connect_clicked({
        let draft = draft.clone();
        move |button| {
            draft.borrow_mut().clear_recent = true;
            button.set_label("Clears on Apply");
            button.set_sensitive(false);
        }
    });
    note(
        &privacy,
        "This controls the Recent list only. Tabs, per-folder view state, and file-operation recovery records are stored separately.",
    );

    let about = page(
        &stack,
        "about",
        "About",
        "A native, keyboard-first, dual-pane file manager.",
    );
    let identity = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    identity.set_margin_top(12);
    identity.set_margin_bottom(18);
    let icon = gtk::Image::from_resource(
        "/org/example/Dualpane/icons/scalable/apps/org.example.Dualpane.svg",
    );
    icon.set_pixel_size(72);
    identity.append(&icon);
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 4);
    labels.set_valign(gtk::Align::Center);
    let name = gtk::Label::new(Some(crate::APP_NAME));
    name.set_xalign(0.0);
    name.add_css_class("title-1");
    labels.append(&name);
    let version = gtk::Label::new(Some(&format!("Version {}", env!("CARGO_PKG_VERSION"))));
    version.set_xalign(0.0);
    version.set_selectable(true);
    version.set_can_focus(false);
    labels.append(&version);
    identity.append(&labels);
    about.append(&identity);
    let updates = updates::Panel::new();
    about.append(&updates.root);
    dialog.connect_closed(move |_| updates.close());
    let creator = gtk::Label::new(None);
    creator.set_markup("<a href=\"https://github.com/aheinze\">Artur Heinze</a>");
    creator.set_xalign(1.0);
    creator.set_tooltip_text(Some("View Artur Heinze’s GitHub profile"));
    row(&about, "Created by", "", &creator);
    info(&about, "License", env!("CARGO_PKG_LICENSE"));
    info(&about, "Built with", "Rust · GTK 4 · libadwaita · Relm4");
    info(
        &about,
        "Platform",
        &format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH),
    );
    info(&about, "Runtime", &runtime_versions());
    let links = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    links.set_margin_top(16);
    links.append(&gtk::LinkButton::with_label(
        "https://github.com/aheinze/Commander",
        "Source code",
    ));
    links.append(&gtk::LinkButton::with_label(
        "https://github.com/aheinze/Commander/issues",
        "Report an issue",
    ));
    about.append(&links);
    let copy = action(
        &about,
        "Troubleshooting",
        "Copy app and runtime versions. No file paths or personal data are included.",
        "Copy details",
    );
    copy.connect_clicked(|button| {
        button.clipboard().set_text(&diagnostics());
        notifications::show(notifications::Kind::Success, "Diagnostics copied");
    });
    note(&about, "Lucide icons include ISC and MIT license notices.");

    let actions = dialog_actions();
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&apply);
    root.append(&actions);
    view.set_content(Some(&root));
    let ui = Rc::new(SettingsUi {
        draft,
        stack,
        search,
        shortcuts,
        apply,
        cancel,
    });
    populate_shortcuts(&ui);
    ui.stack.connect_visible_child_name_notify({
        let ui = Rc::downgrade(&ui);
        move |stack| {
            if stack.visible_child_name().as_deref() == Some("keyboard")
                && let Some(ui) = ui.upgrade()
            {
                ui.search.grab_focus();
            }
        }
    });
    ui.search.connect_search_changed({
        let ui = Rc::downgrade(&ui);
        move |_| {
            if let Some(ui) = ui.upgrade() {
                populate_shortcuts(&ui);
            }
        }
    });
    profile.connect_selected_notify({
        let ui = Rc::downgrade(&ui);
        move |picker| {
            if let Some(ui) = ui.upgrade() {
                ui.draft.borrow_mut().profile = if picker.selected() == 1 {
                    KeymapProfile::Modern
                } else {
                    KeymapProfile::Classic
                };
                populate_shortcuts(&ui);
            }
        }
    });
    reset.connect_clicked({
        let ui = Rc::downgrade(&ui);
        move |_| {
            if let Some(ui) = ui.upgrade() {
                let mut draft = ui.draft.borrow_mut();
                match draft.profile {
                    KeymapProfile::Classic => draft.overrides.classic.clear(),
                    KeymapProfile::Modern => draft.overrides.modern.clear(),
                };
                drop(draft);
                populate_shortcuts(&ui);
            }
        }
    });
    ui.cancel.connect_clicked({
        let dialog = dialog.downgrade();
        move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        }
    });
    ui.apply.connect_clicked({
        let ui = Rc::downgrade(&ui);
        let dialog = dialog.downgrade();
        move |_| {
            if let (Some(ui), Some(dialog)) = (ui.upgrade(), dialog.upgrade()) {
                let _ = input.send(AppMsg::SetSettings(Box::new(ui.draft.borrow().clone())));
                dialog.close();
            }
        }
    });
    // The dialog owns the controller for as long as it is presented; no global state.
    dialog.connect_closed({
        let ui = ui.clone();
        move |_| {
            ui.apply.set_sensitive(false);
        }
    });
    (dialog, ui)
}

fn page(stack: &gtk::Stack, id: &str, title: &str, subtitle: &str) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.add_css_class("settings-page");
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.add_css_class("title-2");
    page.append(&heading);
    note(&page, subtitle);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .overlay_scrolling(false)
        .child(&page)
        .build();
    stack.add_titled(&scroll, Some(id), title);
    page
}

fn section(page: &gtk::Box, title: &str) {
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    label.add_css_class("settings-section");
    page.append(&label);
}

fn note(page: &gtk::Box, text: &str) {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.add_css_class("settings-description");
    page.append(&label);
}

fn row(page: &gtk::Box, title: &str, subtitle: &str, control: &impl IsA<gtk::Widget>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("settings-row");
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 4);
    labels.set_hexpand(true);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_wrap(true);
    name.add_css_class("settings-row-title");
    labels.append(&name);
    if !subtitle.is_empty() {
        note(&labels, subtitle);
    }
    row.append(&labels);
    control.set_valign(gtk::Align::Center);
    if let Some(button) = control.as_ref().downcast_ref::<gtk::Button>() {
        let label = format!("{}: {title}", button.label().unwrap_or_default());
        button.update_property(&[gtk::accessible::Property::Label(&label)]);
    } else if !control.as_ref().is::<gtk::Label>() {
        control
            .as_ref()
            .update_property(&[gtk::accessible::Property::Label(title)]);
    }
    row.append(control);
    page.append(&row);
}

fn toggle(
    page: &gtk::Box,
    title: &str,
    subtitle: &str,
    value: bool,
    changed: impl Fn(bool) + 'static,
) -> gtk::Switch {
    let switch = gtk::Switch::new();
    switch.set_active(value);
    switch.connect_active_notify(move |switch| changed(switch.is_active()));
    row(page, title, subtitle, &switch);
    switch
}

fn action(page: &gtk::Box, title: &str, subtitle: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    row(page, title, subtitle, &button);
    button
}

fn info(page: &gtk::Box, title: &str, value: &str) {
    let label = gtk::Label::new(Some(value));
    label.set_wrap(true);
    label.set_selectable(true);
    label.set_can_focus(false);
    label.set_xalign(1.0);
    row(page, title, "", &label);
}

fn populate_shortcuts(ui: &Rc<SettingsUi>) {
    while let Some(child) = ui.shortcuts.first_child() {
        ui.shortcuts.remove(&child);
    }
    let keymap = ui.draft.borrow().keymap();
    let query = ui.search.text();
    for definition in COMMANDS {
        let binding = keymap.binding_label(definition.id);
        if !palette_query_matches(
            &query,
            &[definition.label, definition.id.as_str(), &binding],
        ) {
            continue;
        }
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let button = gtk::Button::with_label(if binding.is_empty() {
            "Unassigned"
        } else {
            &binding
        });
        button.add_css_class("settings-binding");
        button.set_tooltip_text(Some(&format!("Change shortcut for {}", definition.label)));
        row(&container, definition.label, "", &button);
        ui.shortcuts.append(&container);
        button.connect_clicked({
            let ui = Rc::downgrade(ui);
            let command = definition.id;
            move |button| {
                if let Some(ui) = ui.upgrade() {
                    capture_shortcut(&ui, command, button);
                }
            }
        });
    }
}

fn capture_shortcut(ui: &Rc<SettingsUi>, command: CommandId, parent: &gtk::Button) {
    let title = COMMANDS
        .iter()
        .find(|definition| definition.id == command)
        .unwrap()
        .label;
    let (dialog, view) = utility_dialog(title, 440, 300, "shortcut-capture-dialog");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.add_css_class("dialog-body");
    note(
        &root,
        "Press a key combination below, then Enter to assign. Tab moves between controls; Escape cancels. This replaces the action’s current shortcuts.",
    );
    let capture = gtk::Button::with_label("Press shortcut…");
    capture.add_css_class("shortcut-recorder");
    capture.set_hexpand(true);
    root.append(&capture);
    let hint = gtk::Label::new(Some(
        "Use modifiers such as Ctrl, Alt, or Shift, or a function key.",
    ));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    root.append(&hint);
    let actions = dialog_actions();
    let clear = gtk::Button::with_label("Remove");
    let cancel = gtk::Button::with_label("Cancel");
    let assign = gtk::Button::with_label("Assign");
    assign.add_css_class("suggested-action");
    assign.set_sensitive(false);
    actions.append(&clear);
    actions.append(&cancel);
    actions.append(&assign);
    root.append(&actions);
    view.set_content(Some(&root));
    let pending = Rc::new(RefCell::new(None::<String>));
    let keymap = ui.draft.borrow().keymap();
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let pending = pending.clone();
        let capture = capture.downgrade();
        let assign = assign.downgrade();
        let dialog = dialog.downgrade();
        move |_, key, _, modifiers| {
            let (Some(capture), Some(assign), Some(dialog)) =
                (capture.upgrade(), assign.upgrade(), dialog.upgrade())
            else {
                return glib::Propagation::Proceed;
            };
            if key == gdk::Key::Escape {
                dialog.close();
                return glib::Propagation::Stop;
            }
            if matches!(key, gdk::Key::Tab | gdk::Key::ISO_Left_Tab)
                && !modifiers.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK,
                )
            {
                return glib::Propagation::Proceed;
            }
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) && pending.borrow().is_some() {
                assign.emit_clicked();
                return glib::Propagation::Stop;
            }
            if matches!(
                key,
                gdk::Key::Shift_L
                    | gdk::Key::Shift_R
                    | gdk::Key::Control_L
                    | gdk::Key::Control_R
                    | gdk::Key::Alt_L
                    | gdk::Key::Alt_R
                    | gdk::Key::Super_L
                    | gdk::Key::Super_R
                    | gdk::Key::Meta_L
                    | gdk::Key::Meta_R
            ) {
                return glib::Propagation::Stop;
            }
            let modifiers = modifiers & gtk::accelerator_get_default_mod_mask();
            if !gtk::accelerator_valid(key, modifiers) {
                notifications::error(
                    "That key is reserved for navigation. Try adding Ctrl or Alt.",
                );
                assign.set_sensitive(false);
                *pending.borrow_mut() = None;
                return glib::Propagation::Stop;
            }
            let accelerator = gtk::accelerator_name(key, modifiers).to_string();
            capture.set_label(&gtk::accelerator_get_label(key, modifiers));
            let conflict = keymap
                .command_for(key, modifiers)
                .filter(|other| *other != command)
                .and_then(|other| COMMANDS.iter().find(|definition| definition.id == other));
            notifications::info(&conflict.map_or_else(
                || "Ready to assign. Changes take effect when you Apply settings.".to_owned(),
                |definition| {
                    format!(
                        "Used by “{}”. Assign moves this shortcut to “{title}”.",
                        definition.label
                    )
                },
            ));
            *pending.borrow_mut() = Some(accelerator);
            assign.set_sensitive(true);
            glib::Propagation::Stop
        }
    });
    capture.add_controller(keys);
    let change = {
        let ui = ui.clone();
        move |accelerator: Option<&str>| {
            let keymap = ui.draft.borrow().keymap();
            keymap.assign(command, accelerator);
            ui.draft.borrow_mut().overrides = keymap.overrides();
            populate_shortcuts(&ui);
        }
    };
    let change = Rc::new(change);
    clear.connect_clicked({
        let change = change.clone();
        let dialog = dialog.downgrade();
        move |_| {
            change(None);
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        }
    });
    assign.connect_clicked({
        let dialog = dialog.downgrade();
        move |_| {
            if let Some(value) = pending.borrow().as_deref() {
                change(Some(value));
                if let Some(dialog) = dialog.upgrade() {
                    dialog.close();
                }
            }
        }
    });
    cancel.connect_clicked({
        let dialog = dialog.downgrade();
        move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        }
    });
    dialog.set_focus(Some(&capture));
    dialog.present(Some(parent));
}

fn runtime_versions() -> String {
    format!(
        "GTK {}.{}.{} · libadwaita {}.{}.{}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version()
    )
}

fn diagnostics() -> String {
    format!(
        "Commander {}\n{} / {}\n{}\nLicense: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        runtime_versions(),
        env!("CARGO_PKG_LICENSE")
    )
}
