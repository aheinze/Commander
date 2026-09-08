//! Modal dialogs. Every dialog reports its outcome back through [`AppMsg`].

use super::*;

#[path = "custom_tools.rs"]
mod custom_tools;

pub(super) fn show_new_directory_dialog(sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("New Folder"),
        Some("Create a folder in the active pane."),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("create"));
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    let entry = gtk::Entry::new();
    entry.set_text("Untitled Folder");
    entry.set_activates_default(true);
    entry.select_region(0, -1);
    dialog.set_extra_child(Some(&entry));
    let input = sender.input_sender().clone();
    let response_entry = entry.clone();
    dialog.connect_response(Some("create"), move |_, _| {
        let _ = input.send(AppMsg::CreateDirectory(response_entry.text().to_string()));
    });
    dialog.present(Some(&window));
    entry.grab_focus();
    entry.select_region(0, -1);
}

pub(super) fn show_save_workspace_dialog(sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Save Current Workspace"),
        Some(
            "Store pane tabs, view settings, panel layout, sidebar, and inspector state as a reusable workspace. Saving an existing name updates it.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("save", "Save");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("save"));
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    let name = gtk::Entry::new();
    name.set_text("Workspace");
    name.set_activates_default(true);
    dialog.set_extra_child(Some(&name));
    let input = sender.input_sender().clone();
    let response_name = name.clone();
    dialog.connect_response(Some("save"), move |_, _| {
        let _ = input.send(AppMsg::SaveWorkspace(response_name.text().to_string()));
    });
    dialog.present(Some(&window));
    name.grab_focus();
    name.select_region(0, -1);
}

pub(super) fn show_update_workspace_dialog(index: usize, name: &str, input: relm4::Sender<AppMsg>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Update Workspace?"),
        Some(&format!(
            "Replace the saved “{name}” setup with the current pane tabs and layout?"
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("update", "Update");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("update"));
    dialog.set_response_appearance("update", adw::ResponseAppearance::Suggested);
    dialog.connect_response(Some("update"), move |_, _| {
        let _ = input.send(AppMsg::UpdateWorkspace(index));
    });
    dialog.present(Some(&window));
}

pub(super) fn show_rename_workspace_dialog(index: usize, name: &str, input: relm4::Sender<AppMsg>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Rename Workspace"),
        Some("Choose a unique name for this saved setup."),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("rename", "Rename");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("rename"));
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    let entry = gtk::Entry::new();
    entry.set_text(name);
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    let response_entry = entry.clone();
    dialog.connect_response(Some("rename"), move |_, _| {
        let _ = input.send(AppMsg::RenameWorkspace {
            index,
            name: response_entry.text().to_string(),
        });
    });
    dialog.present(Some(&window));
    entry.grab_focus();
    entry.select_region(0, -1);
}

pub(super) fn show_delete_workspace_dialog(index: usize, name: &str, input: relm4::Sender<AppMsg>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Delete Workspace?"),
        Some(&format!(
            "Remove “{name}”? Your files and folders will not be changed."
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("cancel"));
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.connect_response(Some("delete"), move |_, _| {
        let _ = input.send(AppMsg::RemoveWorkspace(index));
    });
    dialog.present(Some(&window));
}

pub(super) fn show_new_favorite_group_dialog(sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("New Favorite Group"),
        Some("Create a named section in the Favorites sidebar."),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("create"));
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    let name = gtk::Entry::new();
    name.set_placeholder_text(Some("Projects"));
    name.set_activates_default(true);
    dialog.set_extra_child(Some(&name));
    let input = sender.input_sender().clone();
    let response_name = name.clone();
    dialog.connect_response(Some("create"), move |_, _| {
        let _ = input.send(AppMsg::CreateFavoriteGroup(
            response_name.text().to_string(),
        ));
    });
    dialog.present(Some(&window));
    name.grab_focus();
}

pub(super) fn show_rename_favorite_dialog(
    group: Option<usize>,
    path: &VPath,
    name: &str,
    input: relm4::Sender<AppMsg>,
) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Rename Favorite"),
        Some("Change this shortcut’s label. The folder name stays the same."),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("rename", "Rename");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("rename"));
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let entry = gtk::Entry::new();
    entry.update_property(&[gtk::accessible::Property::Label("Favorite name")]);
    entry.set_text(name);
    entry.set_activates_default(true);
    content.append(&entry);
    let hint = gtk::Label::new(Some("Leave blank to use the folder name."));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("alert-body");
    content.append(&hint);
    dialog.set_extra_child(Some(&content));
    let path = path.clone();
    let response_entry = entry.clone();
    dialog.connect_response(Some("rename"), move |_, _| {
        let _ = input.send(AppMsg::RenameFavorite {
            group,
            path: path.clone(),
            name: response_entry.text().to_string(),
        });
    });
    dialog.present(Some(&window));
    entry.grab_focus();
    entry.select_region(0, -1);
}

pub(super) fn connect_remote_uri(
    connection: super::remote::RemoteConnection,
    sender: &ComponentSender<AppModel>,
) {
    let input = sender.input_sender().clone();
    glib::spawn_future_local(async move {
        let uri = connection.uri.clone();
        let result = super::remote::mount_connection(connection).await;
        let _ = input.send(AppMsg::RemoteConnected { uri, result });
    });
}

pub(super) fn show_settings_dialog(
    appearance: AppearanceMode,
    color_theme: ColorTheme,
    parallel_transfers: bool,
    sender: &ComponentSender<AppModel>,
) {
    let Some(parent) = relm4::main_application().active_window() else {
        return;
    };
    let (dialog, view) = utility_dialog("Settings", 440, -1, "settings-dialog");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.add_css_class("dialog-body");
    let intro = gtk::Label::new(Some("Tune the native appearance and transfer behavior."));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.add_css_class("dim-label");
    content.append(&intro);
    let label = gtk::Label::new(Some("Appearance"));
    label.set_xalign(0.0);
    label.add_css_class("dialog-eyebrow");
    let appearance_picker = gtk::DropDown::from_strings(&["System", "Light", "Dark"]);
    appearance_picker.set_selected(match appearance {
        AppearanceMode::System => 0,
        AppearanceMode::Light => 1,
        AppearanceMode::Dark => 2,
    });
    content.append(&label);
    content.append(&appearance_picker);
    let theme_label = gtk::Label::new(Some("Color theme"));
    theme_label.set_xalign(0.0);
    theme_label.add_css_class("dialog-eyebrow");
    let theme_picker = gtk::DropDown::from_strings(&[
        "Automatic (Omarchy)",
        "Carelo Graphite",
        "Midnight Blue",
        "Forest",
        "Aubergine",
    ]);
    theme_picker.set_selected(match color_theme {
        ColorTheme::Automatic => 0,
        ColorTheme::Carelo => 1,
        ColorTheme::Midnight => 2,
        ColorTheme::Forest => 3,
        ColorTheme::Aubergine => 4,
    });
    content.append(&theme_label);
    content.append(&theme_picker);
    let automatic_theme_hint = omarchy::current_theme_name().map_or_else(
        || "Uses Carelo Graphite when an active Omarchy palette is not available.".to_owned(),
        |name| format!("Following Omarchy’s {name} palette. Theme changes apply automatically."),
    );
    let theme_hint = gtk::Label::new(Some(&automatic_theme_hint));
    theme_hint.set_xalign(0.0);
    theme_hint.set_wrap(true);
    theme_hint.add_css_class("dim-label");
    content.append(&theme_hint);
    let transfers_label = gtk::Label::new(Some("Transfers"));
    transfers_label.set_xalign(0.0);
    transfers_label.add_css_class("dialog-eyebrow");
    content.append(&transfers_label);
    let parallel = gtk::CheckButton::with_label("Parallel, storage-aware transfers");
    parallel.set_active(parallel_transfers);
    parallel.set_tooltip_text(Some(
        "Disable to process file payloads sequentially while preserving the job queue",
    ));
    content.append(&parallel);
    let custom_tools_button = gtk::Button::with_label("Manage Context Menu Tools…");
    custom_tools_button.set_halign(gtk::Align::Start);
    custom_tools_button.set_margin_top(6);
    custom_tools_button.add_css_class("flat");
    custom_tools_button.add_css_class("settings-link");
    {
        let input = sender.input_sender().clone();
        custom_tools_button.connect_clicked(move |_| {
            let _ = input.send(AppMsg::ManageCustomTools);
        });
    }
    content.append(&custom_tools_button);
    let shortcuts = gtk::Label::new(Some(
        "Shortcut overrides can be placed in Commander's keymaps.toml file. Press F1 to inspect the active map.",
    ));
    shortcuts.set_wrap(true);
    shortcuts.set_xalign(0.0);
    shortcuts.add_css_class("dim-label");
    content.append(&shortcuts);
    root.append(&content);
    let actions = dialog_actions();
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&apply);
    root.append(&actions);
    view.set_content(Some(&root));
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    let input = sender.input_sender().clone();
    let response_dialog = dialog.clone();
    apply.connect_clicked(move |_| {
        let appearance = match appearance_picker.selected() {
            0 => AppearanceMode::System,
            1 => AppearanceMode::Light,
            _ => AppearanceMode::Dark,
        };
        let color_theme = match theme_picker.selected() {
            1 => ColorTheme::Carelo,
            2 => ColorTheme::Midnight,
            3 => ColorTheme::Forest,
            4 => ColorTheme::Aubergine,
            _ => ColorTheme::Automatic,
        };
        let _ = input.send(AppMsg::SetSettings {
            appearance,
            color_theme,
            parallel_transfers: parallel.is_active(),
        });
        response_dialog.close();
    });
    dialog.present(Some(&parent));
}

pub(super) fn show_custom_tools_dialog(
    tools: Vec<CustomToolSession>,
    sender: &ComponentSender<AppModel>,
) {
    let Some(parent) = relm4::main_application().active_window() else {
        return;
    };
    custom_tools::show(&parent, tools, sender.input_sender().clone());
}

pub(super) fn show_shortcut_reference(keymap: &Keymap) {
    let Some(parent) = relm4::main_application().active_window() else {
        return;
    };
    let (dialog, view) = utility_dialog("Keyboard Shortcuts", 540, 620, "shortcuts-dialog");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let search_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    search_bar.add_css_class("dialog-search-bar");
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Filter shortcuts…"));
    search.set_hexpand(true);
    search.add_css_class("dialog-search");
    search_bar.append(&search);
    root.append(&search_bar);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.add_css_class("shortcut-list");
    let mut searchable_rows = Vec::new();
    for definition in COMMANDS.iter().filter(|definition| definition.available) {
        let binding_text = keymap.binding_label(definition.id);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("shortcut-row");
        let label = gtk::Label::new(Some(definition.label));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        row.append(&label);
        if binding_text.is_empty() {
            let unbound = gtk::Label::new(Some("Not bound"));
            unbound.add_css_class("shortcut-unbound");
            row.append(&unbound);
        } else {
            row.append(&super::widgets::binding_keycaps(&binding_text));
        }
        rows.append(&row);
        searchable_rows.push((row, definition.label.to_ascii_lowercase(), binding_text));
    }
    search.connect_search_changed(move |search| {
        let query = search.text().to_ascii_lowercase();
        for (row, label, binding) in &searchable_rows {
            row.set_visible(
                query.is_empty()
                    || label.contains(&query)
                    || binding.to_ascii_lowercase().contains(&query),
            );
        }
    });
    {
        let dialog = dialog.clone();
        search.connect_stop_search(move |_| {
            dialog.close();
        });
    }
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&rows)
        .build();
    root.append(&scrolled);
    view.set_content(Some(&root));
    dialog.present(Some(&parent));
    search.grab_focus();
}

/// A floating sheet with a flat header bar, shared by the utility dialogs.
///
/// The header bar inherits the dialog title and its close button; Escape closes
/// the sheet for free.
pub(super) fn utility_dialog(
    title: &str,
    width: i32,
    height: i32,
    class: &str,
) -> (adw::Dialog, adw::ToolbarView) {
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(width)
        .presentation_mode(adw::DialogPresentationMode::Floating)
        .build();
    if height > 0 {
        dialog.set_content_height(height);
    }
    dialog.add_css_class("utility-dialog");
    dialog.add_css_class(class);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    dialog.set_child(Some(&view));
    (dialog, view)
}

/// The right-aligned button row that closes a utility dialog.
pub(super) fn dialog_actions() -> gtk::Box {
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("dialog-actions");
    actions.set_halign(gtk::Align::Fill);
    actions.set_hexpand(true);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    actions.append(&spacer);
    actions
}

pub(super) fn show_new_file_dialog(sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(Some("New File"), Some("Create a file in the active pane."));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("create"));
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    let entry = gtk::Entry::new();
    entry.set_text("untitled.txt");
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    let input = sender.input_sender().clone();
    let response_entry = entry.clone();
    dialog.connect_response(Some("create"), move |_, _| {
        let _ = input.send(AppMsg::CreateFile(response_entry.text().to_string()));
    });
    dialog.present(Some(&window));
    entry.grab_focus();
    entry.select_region(0, -1);
}

pub(super) fn show_create_archive_dialog(sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Create Archive"),
        Some("Create an archive beside the selected items."),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("create"));
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let name = gtk::Entry::new();
    name.set_text("Archive");
    name.set_placeholder_text(Some("Archive name"));
    name.set_activates_default(true);
    let format = gtk::DropDown::from_strings(&["ZIP", "7Z", "TAR", "TAR.GZ"]);
    content.append(&name);
    content.append(&format);
    dialog.set_extra_child(Some(&content));
    let input = sender.input_sender().clone();
    let response_name = name.clone();
    dialog.connect_response(Some("create"), move |_, _| {
        let format = match format.selected() {
            1 => ArchiveFormat::SevenZ,
            2 => ArchiveFormat::Tar,
            3 => ArchiveFormat::TarGz,
            _ => ArchiveFormat::Zip,
        };
        let _ = input.send(AppMsg::CreateArchive {
            name: response_name.text().to_string(),
            format,
        });
    });
    dialog.present(Some(&window));
    name.grab_focus();
    name.select_region(0, -1);
}

pub(super) fn show_image_conversion_dialog(sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Convert Image"),
        Some("Create a converted copy next to the source image."),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("convert", "Convert");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("convert"));
    dialog.set_response_appearance("convert", adw::ResponseAppearance::Suggested);
    let format = gtk::DropDown::from_strings(&["PNG", "JPEG", "WebP", "BMP"]);
    dialog.set_extra_child(Some(&format));
    let input = sender.input_sender().clone();
    dialog.connect_response(Some("convert"), move |_, _| {
        let format = match format.selected() {
            1 => ImageOutputFormat::Jpeg,
            2 => ImageOutputFormat::WebP,
            3 => ImageOutputFormat::Bmp,
            _ => ImageOutputFormat::Png,
        };
        let _ = input.send(AppMsg::ConvertImage(format));
    });
    dialog.present(Some(&window));
}

pub(super) fn show_pdf_tools_dialog(selection_count: usize, sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("PDF Tools"),
        Some(&format!(
            "Process {selection_count} selected PDF(s) using native Rust PDF tools."
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("run", "Run");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("run"));
    dialog.set_response_appearance("run", adw::ResponseAppearance::Suggested);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_size_request(500, -1);
    let tool = gtk::DropDown::from_strings(&[
        "Compress",
        "Merge",
        "Extract Pages",
        "Split into Pages",
        "Rotate Pages",
        "Unlock",
    ]);
    content.append(&form_row("Tool", &tool));
    let hint = gtk::Label::new(Some(
        "Compress rewrites and deduplicates PDF streams. Merge uses the current selection order.",
    ));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("dim-label");
    content.append(&hint);
    let range_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let range_label = gtk::Label::new(Some("Pages"));
    range_label.set_width_chars(10);
    range_label.set_xalign(0.0);
    let ranges = gtk::Entry::new();
    ranges.set_placeholder_text(Some("1-3,5 · blank means all for rotate"));
    ranges.set_hexpand(true);
    range_row.append(&range_label);
    range_row.append(&ranges);
    range_row.set_visible(false);
    content.append(&range_row);
    let rotation = gtk::DropDown::from_strings(&["90°", "180°", "270°"]);
    let rotation_row = form_row("Rotation", &rotation);
    rotation_row.set_visible(false);
    content.append(&rotation_row);
    let password = gtk::PasswordEntry::new();
    password.set_show_peek_icon(true);
    let password_row = form_row("Password", &password);
    password_row.set_visible(false);
    content.append(&password_row);
    let other_pane = gtk::CheckButton::with_label("Save output in the other pane");
    content.append(&other_pane);
    dialog.set_extra_child(Some(&content));
    {
        let range_row = range_row.clone();
        let rotation_row = rotation_row.clone();
        let password_row = password_row.clone();
        let hint = hint.clone();
        tool.connect_selected_notify(move |tool| {
            let selected = tool.selected();
            range_row.set_visible(matches!(selected, 2 | 4));
            rotation_row.set_visible(selected == 4);
            password_row.set_visible(selected == 5);
            hint.set_label(match selected {
                0 => "Rewrite and compress PDF object streams.",
                1 => "Combine selected PDFs in their current selection order.",
                2 => "Save a page range such as 1-3,5 as a new PDF.",
                3 => "Create one PDF for every page.",
                4 => "Rotate all pages, or only the entered page range.",
                5 => "Remove encryption using a known password.",
                _ => "PDF tools",
            });
        });
    }
    let input = sender.input_sender().clone();
    dialog.connect_response(Some("run"), move |_, _| {
        let selected = tool.selected();
        let options = PdfToolOptions {
            tool: match selected {
                1 => PdfTool::Merge,
                2 => PdfTool::ExtractPages,
                3 => PdfTool::SplitPages,
                4 => PdfTool::RotatePages,
                5 => PdfTool::Unlock,
                _ => PdfTool::Compress,
            },
            page_ranges: ranges.text().to_string(),
            rotation: match rotation.selected() {
                1 => 180,
                2 => 270,
                _ => 90,
            },
            password: password.text().to_string(),
        };
        let _ = input.send(AppMsg::RunPdfTool {
            options,
            other_pane: other_pane.is_active(),
        });
    });
    dialog.present(Some(&window));
}

pub(super) fn show_rename_dialog(source: VPath, sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(Some("Rename"), Some("Enter a new name for this item."));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("rename", "Rename");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("rename"));
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    let entry = gtk::Entry::new();
    entry.set_text(
        &source
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
    );
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    let input = sender.input_sender().clone();
    let response_entry = entry.clone();
    dialog.connect_response(Some("rename"), move |_, _| {
        let _ = input.send(AppMsg::RenamePath(
            source.clone(),
            response_entry.text().to_string(),
        ));
    });
    dialog.present(Some(&window));
    entry.grab_focus();
    entry.select_region(0, -1);
}

pub(super) fn show_batch_rename_dialog(sources: Vec<VPath>, sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Batch Rename"),
        Some(&format!(
            "Rename {} selected items with a shared rule.",
            sources.len()
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("rename", "Rename Items");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("rename"));
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_size_request(500, -1);
    let method = gtk::DropDown::from_strings(&["Replace", "Add Text", "Number", "Change Case"]);
    content.append(&form_row("Method", &method));

    let replace_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let find = gtk::Entry::new();
    find.set_placeholder_text(Some("Text to find"));
    let replacement = gtk::Entry::new();
    replacement.set_placeholder_text(Some("Replacement"));
    let match_case = gtk::CheckButton::with_label("Match case");
    replace_box.append(&form_row("Find", &find));
    replace_box.append(&form_row("Replace", &replacement));
    replace_box.append(&match_case);
    content.append(&replace_box);

    let add_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let prefix = gtk::Entry::new();
    prefix.set_placeholder_text(Some("Prefix"));
    let suffix = gtk::Entry::new();
    suffix.set_placeholder_text(Some("Suffix"));
    add_box.append(&form_row("Prefix", &prefix));
    add_box.append(&form_row("Suffix", &suffix));
    add_box.set_visible(false);
    content.append(&add_box);

    let number_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let template = gtk::Entry::new();
    template.set_text("{name} {n}");
    template.set_tooltip_text(Some(
        "Use {name} for the original name and {n} for the number",
    ));
    let start = gtk::SpinButton::with_range(0.0, 999_999.0, 1.0);
    start.set_value(1.0);
    let padding = gtk::SpinButton::with_range(1.0, 8.0, 1.0);
    padding.set_value(2.0);
    number_box.append(&form_row("Template", &template));
    number_box.append(&form_row("Start", &start));
    number_box.append(&form_row("Digits", &padding));
    number_box.set_visible(false);
    content.append(&number_box);

    let case_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let case_mode = gtk::DropDown::from_strings(&["lowercase", "UPPERCASE", "Title Case"]);
    case_box.append(&form_row("Case", &case_mode));
    case_box.set_visible(false);
    content.append(&case_box);
    let keep_extensions = gtk::CheckButton::with_label("Keep file extensions unchanged");
    keep_extensions.set_active(true);
    content.append(&keep_extensions);
    let preview = gtk::Label::new(Some(
        "Replace text, add a prefix/suffix, append a sequence, or normalize casing.",
    ));
    preview.set_wrap(true);
    preview.set_xalign(0.0);
    preview.add_css_class("dim-label");
    content.append(&preview);
    dialog.set_extra_child(Some(&content));

    {
        let replace_box = replace_box.clone();
        let add_box = add_box.clone();
        let number_box = number_box.clone();
        let case_box = case_box.clone();
        method.connect_selected_notify(move |method| {
            let selected = method.selected();
            replace_box.set_visible(selected == 0);
            add_box.set_visible(selected == 1);
            number_box.set_visible(selected == 2);
            case_box.set_visible(selected == 3);
        });
    }
    let input = sender.input_sender().clone();
    let response_find = find.clone();
    dialog.connect_response(Some("rename"), move |_, _| {
        let selected = method.selected();
        let keep_extensions = keep_extensions.is_active();
        let padding = usize::try_from(padding.value_as_int()).unwrap_or(2);
        let start_number = start.value_as_int();
        let items = sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let original = source
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                let (stem, extension) = split_extension(&original, keep_extensions);
                let next_stem = match selected {
                    0 => replace_text(
                        &stem,
                        &response_find.text(),
                        &replacement.text(),
                        match_case.is_active(),
                    ),
                    1 => format!("{}{}{}", prefix.text(), stem, suffix.text()),
                    2 => {
                        let number = i64::from(start_number) + i64::try_from(index).unwrap_or(0);
                        template
                            .text()
                            .replace("{name}", &stem)
                            .replace("{n}", &format!("{number:0padding$}"))
                    }
                    3 => match case_mode.selected() {
                        1 => stem.to_uppercase(),
                        2 => title_case(&stem),
                        _ => stem.to_lowercase(),
                    },
                    _ => stem,
                };
                (source.clone(), format!("{next_stem}{extension}"))
            })
            .collect();
        let _ = input.send(AppMsg::BatchRename(items));
    });
    dialog.present(Some(&window));
    find.grab_focus();
}

pub(super) fn form_row(label: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let label = gtk::Label::new(Some(label));
    label.set_width_chars(10);
    label.set_xalign(0.0);
    child.set_hexpand(true);
    row.append(&label);
    row.append(child);
    row
}

pub(super) fn show_open_with_dialog(path: VPath) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let (content_type, _) = gio::content_type_guess(Some(path.as_path()), None);
    let recommended = gio::AppInfo::recommended_for_type(&content_type);
    let recommended_ids: Vec<_> = recommended.iter().map(gio::AppInfo::id).collect();
    let others: Vec<_> = gio::AppInfo::all_for_type(&content_type)
        .into_iter()
        .filter(|app| !recommended_ids.contains(&app.id()))
        .collect();
    let file_name = path.file_name().map_or_else(
        || "this item".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let (dialog, view) = utility_dialog("Open With", 420, 520, "open-with-dialog");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let intro = gtk::Label::new(Some(&format!(
        "{file_name} · {}",
        gio::content_type_get_description(&content_type)
    )));
    intro.set_xalign(0.0);
    intro.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    intro.add_css_class("dim-label");
    intro.add_css_class("open-with-intro");
    root.append(&intro);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 1);
    list.add_css_class("open-with-list");
    let mut sections = vec![("Recommended", recommended), ("Other applications", others)];
    if sections.iter().all(|(_, apps)| apps.is_empty()) {
        sections = vec![("All applications", gio::AppInfo::all())];
    }
    let mut listed = false;
    for (heading, apps) in sections {
        if apps.is_empty() {
            continue;
        }
        listed = true;
        let label = gtk::Label::new(Some(heading));
        label.set_xalign(0.0);
        label.add_css_class("dialog-eyebrow");
        label.add_css_class("open-with-section");
        list.append(&label);
        for app in apps {
            list.append(&open_with_row(&app, &path, &dialog));
        }
    }
    if !listed {
        let empty = gtk::Label::new(Some("No applications are available."));
        empty.add_css_class("open-with-empty");
        list.append(&empty);
    }
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&list)
        .build();
    root.append(&scrolled);
    view.set_content(Some(&root));
    dialog.present(Some(&window));
}

/// One launchable application in the Open With sheet.
fn open_with_row(app: &gio::AppInfo, path: &VPath, dialog: &adw::Dialog) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("open-with-row");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = app.icon().map_or_else(
        || gtk::Image::from_icon_name("commander-app-window-symbolic"),
        |icon| gtk::Image::from_gicon(&icon),
    );
    icon.set_pixel_size(24);
    content.append(&icon);
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 1);
    labels.set_hexpand(true);
    let name = gtk::Label::new(Some(&app.display_name()));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.add_css_class("open-with-name");
    labels.append(&name);
    if let Some(description) = app.description() {
        let description = gtk::Label::new(Some(&description));
        description.set_xalign(0.0);
        description.set_ellipsize(gtk::pango::EllipsizeMode::End);
        description.add_css_class("open-with-description");
        labels.append(&description);
    }
    content.append(&labels);
    row.set_child(Some(&content));
    let app = app.clone();
    let file = gio::File::for_path(path.as_path());
    let dialog = dialog.clone();
    row.connect_clicked(move |_| {
        if let Err(error) = app.launch(std::slice::from_ref(&file), None::<&gio::AppLaunchContext>)
        {
            tracing::warn!(%error, "could not launch application");
        }
        dialog.close();
    });
    row
}

pub(super) fn reveal_in_file_manager(path: VPath) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path.as_path())));
    launcher.open_containing_folder(Some(&window), None::<&gio::Cancellable>, |_| {});
}

pub(super) fn show_conflict_dialog(
    pane: PaneId,
    job_id: JobId,
    conflict_id: ConflictId,
    conflict: &Conflict,
    sender: &ComponentSender<AppModel>,
) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let source_modified = conflict.source_metadata.modified.map_or_else(
        || "unknown".to_owned(),
        |value| format_timestamp(value.seconds),
    );
    let destination_modified = conflict.destination_metadata.modified.map_or_else(
        || "unknown".to_owned(),
        |value| format_timestamp(value.seconds),
    );
    let body = format!(
        "A file named “{}” already exists.\n\nSource: {} · {}\nExisting: {} · {}",
        conflict.destination.file_name().map_or_else(
            || "item".to_owned(),
            |name| name.to_string_lossy().into_owned()
        ),
        format_size(conflict.source_metadata.size, conflict.source_metadata.kind),
        source_modified,
        format_size(
            conflict.destination_metadata.size,
            conflict.destination_metadata.kind,
        ),
        destination_modified,
    );
    let dialog = AlertSheet::new(Some("Resolve File Conflict"), Some(&body));
    dialog.add_response("skip", "Skip");
    dialog.add_response("newer", "Replace if Newer");
    dialog.add_response("compare", "Compare Checksums");
    dialog.add_response("keep", "Keep Both");
    dialog.add_response("replace", "Replace");
    dialog.set_close_response("skip");
    dialog.set_default_response(Some("keep"));
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("keep", adw::ResponseAppearance::Suggested);
    let apply_to_all = gtk::CheckButton::with_label("Apply this decision to all conflicts");
    dialog.set_extra_child(Some(&apply_to_all));
    let input = sender.input_sender().clone();
    let checksum_source = conflict.source.clone();
    let checksum_destination = conflict.destination.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "compare" {
            let _ = input.send(AppMsg::CompareConflictChecksum {
                pane,
                job_id,
                conflict_id,
                source: checksum_source.clone(),
                destination: checksum_destination.clone(),
            });
            return;
        }
        let choice = match response {
            "replace" => ConflictChoice::Replace,
            "newer" => ConflictChoice::ReplaceIfNewer,
            "keep" => ConflictChoice::KeepBoth,
            _ => ConflictChoice::Skip,
        };
        let _ = input.send(AppMsg::ResolveConflict {
            job_id,
            conflict_id,
            choice,
            apply_to_all: apply_to_all.is_active(),
        });
    });
    dialog.present(Some(&window));
}

pub(super) fn show_permissions_dialog(
    path: VPath,
    initial_mode: u32,
    sender: &ComponentSender<AppModel>,
) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Permissions"),
        Some(&format!("Change Unix permissions for {path}")),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("apply", "Apply");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("apply"));
    dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(14);
    for (column, label) in ["Read", "Write", "Execute"].iter().enumerate() {
        let heading = gtk::Label::new(Some(label));
        heading.add_css_class("permissions-heading");
        grid.attach(&heading, i32::try_from(column + 1).unwrap_or(0), 0, 1, 1);
    }
    let specs = [
        ("Owner", [0o400, 0o200, 0o100]),
        ("Group", [0o040, 0o020, 0o010]),
        ("Others", [0o004, 0o002, 0o001]),
    ];
    let mut toggles = Vec::new();
    for (row, (label, bits)) in specs.iter().enumerate() {
        let label = gtk::Label::new(Some(label));
        label.set_xalign(0.0);
        grid.attach(&label, 0, i32::try_from(row + 1).unwrap_or(0), 1, 1);
        for (column, bit) in bits.iter().enumerate() {
            let toggle = gtk::CheckButton::new();
            toggle.set_active(initial_mode & bit != 0);
            grid.attach(
                &toggle,
                i32::try_from(column + 1).unwrap_or(0),
                i32::try_from(row + 1).unwrap_or(0),
                1,
                1,
            );
            toggles.push((*bit, toggle));
        }
    }
    content.append(&grid);
    let special = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let mut special_toggles = Vec::new();
    for (label, bit) in [("setuid", 0o4000), ("setgid", 0o2000), ("sticky", 0o1000)] {
        let toggle = gtk::CheckButton::with_label(label);
        toggle.set_active(initial_mode & bit != 0);
        special.append(&toggle);
        special_toggles.push((bit, toggle));
    }
    toggles.extend(special_toggles);
    content.append(&special);
    let octal_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let octal_label = gtk::Label::new(Some("Octal"));
    let octal = gtk::Entry::new();
    octal.set_width_chars(6);
    octal.set_text(&format!("{:04o}", initial_mode & 0o7777));
    let preview = gtk::Label::new(Some(&format_mode(initial_mode)));
    preview.set_hexpand(true);
    preview.set_xalign(0.0);
    preview.add_css_class("monospace");
    octal_row.append(&octal_label);
    octal_row.append(&octal);
    octal_row.append(&preview);
    content.append(&octal_row);
    let recursive = gtk::CheckButton::with_label("Apply recursively to folder contents");
    content.append(&recursive);
    dialog.set_extra_child(Some(&content));

    let mode = Rc::new(Cell::new(initial_mode & 0o7777));
    let updating = Rc::new(Cell::new(false));
    let toggles = Rc::new(toggles);
    for (_, toggle) in toggles.iter() {
        let toggles = Rc::clone(&toggles);
        let mode = Rc::clone(&mode);
        let updating = Rc::clone(&updating);
        let octal = octal.clone();
        let preview = preview.clone();
        toggle.connect_toggled(move |_| {
            if updating.get() {
                return;
            }
            let next = toggles.iter().fold(0_u32, |value, (bit, toggle)| {
                if toggle.is_active() {
                    value | bit
                } else {
                    value
                }
            });
            mode.set(next);
            octal.set_text(&format!("{next:04o}"));
            preview.set_label(&format_mode(next));
        });
    }
    {
        let toggles = Rc::clone(&toggles);
        let mode = Rc::clone(&mode);
        let updating = Rc::clone(&updating);
        let preview = preview.clone();
        octal.connect_changed(move |entry| {
            let Ok(next) = u32::from_str_radix(entry.text().trim(), 8) else {
                entry.add_css_class("error");
                return;
            };
            if next > 0o7777 {
                entry.add_css_class("error");
                return;
            }
            entry.remove_css_class("error");
            mode.set(next);
            preview.set_label(&format_mode(next));
            updating.set(true);
            for (bit, toggle) in toggles.iter() {
                toggle.set_active(next & bit != 0);
            }
            updating.set(false);
        });
    }
    let input = sender.input_sender().clone();
    dialog.connect_response(Some("apply"), move |_, _| {
        let _ = input.send(AppMsg::ApplyPermissions {
            path: path.clone(),
            mode: mode.get(),
            recursive: recursive.is_active(),
        });
    });
    dialog.present(Some(&window));
}

pub(super) fn show_elevated_permissions_dialog(
    path: VPath,
    mode: u32,
    recursive: bool,
    error: &str,
    sender: &ComponentSender<AppModel>,
) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = AlertSheet::new(
        Some("Administrator Permission Required"),
        Some(&format!(
            "The regular permission update was denied. Retry through the system authorization prompt?\n\n{error}"
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("retry", "Retry as Administrator");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("retry"));
    dialog.set_response_appearance("retry", adw::ResponseAppearance::Suggested);
    let input = sender.input_sender().clone();
    dialog.connect_response(Some("retry"), move |_, _| {
        let _ = input.send(AppMsg::ApplyElevatedPermissions {
            path: path.clone(),
            mode,
            recursive,
        });
    });
    dialog.present(Some(&window));
}

pub(super) fn show_checksum_result(path: &VPath, result: Result<String, String>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = match result {
        Ok(checksum) => {
            let dialog = AlertSheet::new(Some("SHA-256 Checksum"), Some(&format!("{}", path)));
            let value = gtk::Entry::new();
            value.set_text(&checksum);
            value.set_editable(false);
            value.set_hexpand(true);
            value.add_css_class("monospace");
            dialog.set_extra_child(Some(&value));
            dialog
        }
        Err(error) => AlertSheet::new(Some("Checksum Failed"), Some(&error)),
    };
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");
    dialog.present(Some(&window));
}

pub(super) fn show_checksum_comparison(
    left: &VPath,
    right: &VPath,
    result: Result<(String, String), String>,
) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let dialog = match result {
        Ok((left_hash, right_hash)) => {
            let matches = left_hash == right_hash;
            let dialog = AlertSheet::new(
                Some(if matches {
                    "Checksums Match"
                } else {
                    "Checksums Differ"
                }),
                Some(if matches {
                    "The two files are byte-for-byte identical."
                } else {
                    "The files have different SHA-256 fingerprints."
                }),
            );
            let values = gtk::Box::new(gtk::Orientation::Vertical, 8);
            for (path, hash) in [(left, left_hash), (right, right_hash)] {
                let name = gtk::Label::new(Some(&path.to_string()));
                name.set_xalign(0.0);
                name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                let value = gtk::Entry::new();
                value.set_text(&hash);
                value.set_editable(false);
                value.add_css_class("monospace");
                values.append(&name);
                values.append(&value);
            }
            dialog.set_extra_child(Some(&values));
            dialog
        }
        Err(error) => AlertSheet::new(Some("Checksum Comparison Failed"), Some(&error)),
    };
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");
    dialog.present(Some(&window));
}

pub(super) fn show_permanent_delete_dialog(count: usize, sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let item = if count == 1 { "item" } else { "items" };
    let body = format!("Permanently delete {count} selected {item}? This action cannot be undone.");
    let dialog = AlertSheet::new(Some("Delete Permanently?"), Some(&body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("cancel"));
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    let input = sender.input_sender().clone();
    dialog.connect_response(Some("delete"), move |_, _| {
        let _ = input.send(AppMsg::DeletePermanentConfirmed);
    });
    dialog.present(Some(&window));
}
