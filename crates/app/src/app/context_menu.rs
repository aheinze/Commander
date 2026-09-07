//! Searchable right-click menus and the toolbar buttons that share their vocabulary.

use super::*;

// Keep the actual non-directory kind on recycled rows: an image extension on a
// FIFO, device, or symlink does not make it a regular image conversion source.
pub(super) const CONTEXT_SPECIAL_KINDS: &[(EntryKind, &str)] = &[
    (EntryKind::Symlink, "context-symlink-target"),
    (EntryKind::Socket, "context-socket-target"),
    (EntryKind::Fifo, "context-fifo-target"),
    (
        EntryKind::CharacterDevice,
        "context-character-device-target",
    ),
    (EntryKind::BlockDevice, "context-block-device-target"),
    (EntryKind::Unknown, "context-unknown-target"),
];

pub(super) fn install_file_context_menu(
    widget: &impl IsA<gtk::Widget>,
    pane: PaneId,
    sender: &ComponentSender<AppModel>,
    keymap: Keymap,
    custom_tools: Rc<RefCell<Vec<CustomToolSession>>>,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(3);
    let widget = widget.as_ref().clone();
    let menu_parent = widget.clone();
    let target_widget = widget.clone();
    let input = sender.input_sender().clone();
    gesture.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        let target = context_target_at(&target_widget, x, y);
        let _ = input.send(AppMsg::ContextTarget(pane, target.clone()));

        let popover = gtk::Popover::new();
        popover.add_css_class("file-context-menu");
        popover.set_autohide(true);
        popover.set_has_arrow(false);
        popover.set_parent(&menu_parent);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

        let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Filter actions…"));
        search.add_css_class("context-menu-search");
        shell.append(&search);

        let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        title.add_css_class("context-menu-title");
        let title_text = target
            .as_ref()
            .and_then(|(path, _)| path.file_name())
            .map_or_else(
                || "Current Folder".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
        let title_label = gtk::Label::new(Some(&title_text));
        title_label.set_xalign(0.0);
        title_label.set_hexpand(true);
        title_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let title_kind = gtk::Label::new(Some(match target.as_ref().map(|(_, kind)| kind) {
            Some(EntryKind::Directory) | None => "Folder",
            Some(_) => "File",
        }));
        title_kind.add_css_class("dim-label");
        title.append(&title_label);
        title.append(&title_kind);
        shell.append(&title);

        let actions = gtk::Box::new(gtk::Orientation::Vertical, 0);
        actions.set_margin_top(2);
        actions.set_margin_bottom(6);
        actions.set_margin_start(6);
        actions.set_margin_end(6);
        let mut groups = Vec::new();
        let empty = gtk::Label::new(Some("No matching actions"));
        empty.add_css_class("context-menu-empty");
        empty.set_visible(false);
        let make_row = |label, icon, fallback_shortcut, command, destructive| {
            let binding = keymap.binding_label(command);
            let shortcut = if binding.is_empty() {
                fallback_shortcut
            } else {
                binding.as_str()
            };
            context_menu_command_row(
                label,
                icon,
                shortcut,
                command,
                destructive,
                &input,
                &popover,
            )
        };

        if let Some((path, kind)) = &target {
            let mut tool_rows = Vec::new();
            if *kind != EntryKind::Directory {
                tool_rows.push(make_row(
                    "Verify Checksum…",
                    "commander-info-symbolic",
                    "",
                    CommandId::Checksum,
                    false,
                ));
                if path
                    .as_path()
                    .extension()
                    .and_then(OsStr::to_str)
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
                {
                    tool_rows.push(make_row(
                        "PDF Tools…",
                        "commander-file-symbolic",
                        "",
                        CommandId::PdfTools,
                        false,
                    ));
                }
                if supports_image_conversion(path, *kind) {
                    tool_rows.push(make_row(
                        "Convert Image…",
                        "commander-layout-grid-symbolic",
                        "",
                        CommandId::ConvertImage,
                        false,
                    ));
                }
            }
            for (index, tool) in custom_tools
                .borrow()
                .iter()
                .enumerate()
                .filter(|(_, tool)| tool.enabled)
            {
                tool_rows.push(context_menu_tool_row(&tool.name, index, &input, &popover));
            }
            append_context_menu_group(&actions, &mut groups, tool_rows);

            let mut open_rows = vec![make_row(
                "Open",
                if *kind == EntryKind::Directory {
                    "commander-folder-symbolic"
                } else {
                    "commander-file-symbolic"
                },
                "Enter",
                CommandId::Open,
                false,
            )];
            if *kind != EntryKind::Directory {
                open_rows.push(make_row(
                    "Edit File",
                    "commander-file-plus-symbolic",
                    "F4",
                    CommandId::EditFile,
                    false,
                ));
                open_rows.push(make_row(
                    "Open With…",
                    "commander-app-window-symbolic",
                    "",
                    CommandId::OpenWith,
                    false,
                ));
            } else {
                open_rows.push(make_row(
                    "Open in New Tab",
                    "commander-square-plus-symbolic",
                    "",
                    CommandId::OpenInNewTab,
                    false,
                ));
            }
            open_rows.extend([
                make_row(
                    "Open in Other Pane",
                    "commander-chevron-right-symbolic",
                    "",
                    CommandId::OpenOtherPane,
                    false,
                ),
                make_row(
                    "Quick Look",
                    "commander-eye-symbolic",
                    "Space",
                    CommandId::QuickLook,
                    false,
                ),
                make_row(
                    "Reveal in File Manager",
                    "commander-folder-symbolic",
                    "",
                    CommandId::Reveal,
                    false,
                ),
            ]);
            append_context_menu_group(&actions, &mut groups, open_rows);

            append_context_menu_group(
                &actions,
                &mut groups,
                vec![
                    make_row(
                        "Copy Path",
                        "commander-copy-symbolic",
                        "Ctrl+Shift+C",
                        CommandId::CopyPath,
                        false,
                    ),
                    make_row(
                        "Cut",
                        "commander-trash-symbolic",
                        "Ctrl+X",
                        CommandId::Cut,
                        false,
                    ),
                    make_row(
                        "Copy",
                        "commander-copy-symbolic",
                        "Ctrl+C",
                        CommandId::CopyClipboard,
                        false,
                    ),
                    make_row(
                        "Paste",
                        "commander-rotate-ccw-clock-symbolic",
                        "Ctrl+V",
                        CommandId::Paste,
                        false,
                    ),
                    make_row(
                        "Rename…",
                        "commander-file-symbolic",
                        "F2",
                        CommandId::Rename,
                        false,
                    ),
                    make_row(
                        "Permissions…",
                        "commander-info-symbolic",
                        "",
                        CommandId::Permissions,
                        false,
                    ),
                    make_row(
                        "Batch Rename…",
                        "commander-file-symbolic",
                        "Ctrl+F2",
                        CommandId::BatchRename,
                        false,
                    ),
                ],
            );

            let mut archive_rows = vec![make_row(
                "Create Archive…",
                "commander-archive-symbolic",
                "",
                CommandId::CreateArchive,
                false,
            )];
            if is_archive_path(path) {
                archive_rows.push(make_row(
                    "Extract Archive",
                    "commander-archive-symbolic",
                    "",
                    CommandId::ExtractArchive,
                    false,
                ));
            }
            append_context_menu_group(&actions, &mut groups, archive_rows);

            append_context_menu_group(
                &actions,
                &mut groups,
                vec![
                    make_row(
                        "Copy to Other Pane",
                        "commander-copy-symbolic",
                        "F5",
                        CommandId::Copy,
                        false,
                    ),
                    make_row(
                        "Move to Other Pane",
                        "commander-chevron-right-symbolic",
                        "F6",
                        CommandId::Move,
                        false,
                    ),
                ],
            );

            let tags = context_menu_tag_row(&input, &popover);
            actions.append(&tags);

            append_context_menu_group(
                &actions,
                &mut groups,
                vec![
                    make_row(
                        "Move to Trash",
                        "commander-trash-symbolic",
                        "F8",
                        CommandId::Trash,
                        false,
                    ),
                    make_row(
                        "Delete Permanently…",
                        "commander-trash-symbolic",
                        "Shift+Delete",
                        CommandId::DeletePermanent,
                        true,
                    ),
                ],
            );

            let filter_groups = groups.clone();
            let filter_tags = tags.clone();
            let filter_empty = empty.clone();
            search.connect_search_changed(move |search| {
                filter_empty
                    .set_visible(!filter_context_menu(search.text().as_str(), &filter_groups));
                filter_tags.set_visible(search.text().trim().is_empty());
            });
        } else {
            append_context_menu_group(
                &actions,
                &mut groups,
                vec![
                    make_row(
                        "New Folder",
                        "commander-folder-plus-symbolic",
                        "F7",
                        CommandId::NewDirectory,
                        false,
                    ),
                    make_row(
                        "New File",
                        "commander-file-plus-symbolic",
                        "",
                        CommandId::NewFile,
                        false,
                    ),
                    make_row(
                        "Refresh Folder",
                        "commander-refresh-cw-symbolic",
                        "Ctrl+R",
                        CommandId::Refresh,
                        false,
                    ),
                ],
            );
            append_context_menu_group(
                &actions,
                &mut groups,
                vec![
                    make_row(
                        "Open in New Tab",
                        "commander-square-plus-symbolic",
                        "Ctrl+T",
                        CommandId::NewTab,
                        false,
                    ),
                    make_row(
                        "Copy Folder Path",
                        "commander-copy-symbolic",
                        "",
                        CommandId::CopyDirectoryPath,
                        false,
                    ),
                    make_row(
                        "Paste",
                        "commander-rotate-ccw-clock-symbolic",
                        "Ctrl+V",
                        CommandId::Paste,
                        false,
                    ),
                ],
            );
            let filter_groups = groups.clone();
            let filter_empty = empty.clone();
            search.connect_search_changed(move |search| {
                filter_empty
                    .set_visible(!filter_context_menu(search.text().as_str(), &filter_groups));
            });
        }
        actions.append(&empty);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .max_content_height(540)
            .propagate_natural_height(true)
            .child(&actions)
            .build();
        scrolled.set_min_content_width(254);
        shell.append(&scrolled);
        popover.set_child(Some(&shell));
        let focus_search = search.clone();
        popover.connect_show(move |_| {
            focus_search.grab_focus();
        });
        popover.connect_closed(|popover| popover.unparent());
        popover.popup();
    });
    widget.add_controller(gesture);
}

#[derive(Clone)]
pub(super) struct ContextMenuFilterGroup {
    pub(super) separator: gtk::Separator,
    pub(super) container: gtk::Box,
    pub(super) rows: Vec<(gtk::Button, String)>,
}

pub(super) fn context_target_at(
    widget: &gtk::Widget,
    x: f64,
    y: f64,
) -> Option<(VPath, EntryKind)> {
    let mut picked = widget.pick(x, y, gtk::PickFlags::DEFAULT)?;
    loop {
        let kind = if picked.has_css_class("context-directory-target") {
            Some(EntryKind::Directory)
        } else if picked.has_css_class("context-file-target") {
            Some(
                CONTEXT_SPECIAL_KINDS
                    .iter()
                    .find(|(_, class)| picked.has_css_class(class))
                    .map_or(EntryKind::File, |(kind, _)| *kind),
            )
        } else {
            None
        };
        if let Some(kind) = kind
            && let Some(path) = picked.tooltip_text()
        {
            return Some((VPath::from(path.as_str()), kind));
        }
        if picked == *widget {
            return None;
        }
        picked = picked.parent()?;
    }
}

pub(super) fn append_context_menu_group(
    parent: &gtk::Box,
    groups: &mut Vec<ContextMenuFilterGroup>,
    rows: Vec<(gtk::Button, String)>,
) {
    if rows.is_empty() {
        return;
    }
    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    separator.add_css_class("context-menu-separator");
    separator.set_visible(!groups.is_empty());
    parent.append(&separator);
    let container = gtk::Box::new(gtk::Orientation::Vertical, 1);
    for (button, _) in &rows {
        container.append(button);
    }
    parent.append(&container);
    groups.push(ContextMenuFilterGroup {
        separator,
        container,
        rows,
    });
}

pub(super) fn filter_context_menu(query: &str, groups: &[ContextMenuFilterGroup]) -> bool {
    let query = query.trim().to_ascii_lowercase();
    let mut has_visible_group = false;
    for group in groups {
        let mut group_visible = false;
        for (button, searchable) in &group.rows {
            let visible = query.is_empty()
                || query
                    .split_whitespace()
                    .all(|term| searchable.contains(term));
            button.set_visible(visible);
            group_visible |= visible;
        }
        group.container.set_visible(group_visible);
        group
            .separator
            .set_visible(group_visible && has_visible_group);
        has_visible_group |= group_visible;
    }
    has_visible_group
}

#[allow(clippy::too_many_arguments)]
pub(super) fn context_menu_command_row(
    label: &str,
    icon: &str,
    shortcut: &str,
    command: CommandId,
    destructive: bool,
    input: &relm4::Sender<AppMsg>,
    popover: &gtk::Popover,
) -> (gtk::Button, String) {
    let button = context_menu_item_button(label, icon, (!shortcut.is_empty()).then_some(shortcut));
    if destructive {
        button.add_css_class("destructive-action");
    }
    let input = input.clone();
    let popover = popover.clone();
    button.connect_clicked(move |_| {
        let _ = input.send(AppMsg::ExecuteCommand(command));
        popover.popdown();
    });
    (
        button,
        format!("{label} {shortcut} {}", command.as_str()).to_ascii_lowercase(),
    )
}

pub(super) fn context_menu_tool_row(
    label: &str,
    index: usize,
    input: &relm4::Sender<AppMsg>,
    popover: &gtk::Popover,
) -> (gtk::Button, String) {
    let button = context_menu_item_button(label, "commander-app-window-symbolic", None);
    let input = input.clone();
    let popover = popover.clone();
    button.connect_clicked(move |_| {
        let _ = input.send(AppMsg::RunCustomTool(index));
        popover.popdown();
    });
    (button, format!("{label} custom tool").to_ascii_lowercase())
}

/// Builds the shared full-width action row used by file and sidebar context menus.
pub(super) fn context_menu_item_button(
    label: &str,
    icon: &str,
    shortcut: Option<&str>,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("context-menu-item");
    button.set_halign(gtk::Align::Fill);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(16);
    image.add_css_class("context-menu-icon");
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    content.append(&image);
    content.append(&text);
    if let Some(shortcut) = shortcut {
        let shortcut_label = gtk::Label::new(Some(shortcut));
        shortcut_label.add_css_class("context-menu-shortcut");
        content.append(&shortcut_label);
    }
    button.set_child(Some(&content));
    button
}

pub(super) fn context_menu_tag_row(
    input: &relm4::Sender<AppMsg>,
    popover: &gtk::Popover,
) -> gtk::Box {
    let tags = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    tags.add_css_class("context-tag-row");
    let tag_label = gtk::Label::new(Some("Tags"));
    tag_label.set_hexpand(true);
    tag_label.set_xalign(0.0);
    tags.append(&tag_label);
    for (color, command) in [
        ("red", CommandId::TagRed),
        ("orange", CommandId::TagOrange),
        ("yellow", CommandId::TagYellow),
        ("green", CommandId::TagGreen),
        ("blue", CommandId::TagBlue),
        ("purple", CommandId::TagPurple),
    ] {
        let button = gtk::Button::new();
        button.add_css_class("context-tag-button");
        button.add_css_class(&format!("tag-swatch-{color}"));
        button.set_size_request(16, 16);
        button.set_halign(gtk::Align::Center);
        button.set_valign(gtk::Align::Center);
        button.set_tooltip_text(Some(color));
        let input = input.clone();
        let popover = popover.clone();
        button.connect_clicked(move |_| {
            let _ = input.send(AppMsg::ExecuteCommand(command));
            popover.popdown();
        });
        tags.append(&button);
    }
    let clear = icon_button("commander-x-symbolic", "Clear tag");
    clear.add_css_class("flat");
    clear.add_css_class("context-tag-clear");
    clear.set_tooltip_text(Some("Clear tag"));
    let input = input.clone();
    let popover = popover.clone();
    clear.connect_clicked(move |_| {
        let _ = input.send(AppMsg::ExecuteCommand(CommandId::ClearTag));
        popover.popdown();
    });
    tags.append(&clear);
    tags
}

pub(super) fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .build();
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

pub(super) fn view_toggle_button(icon: &str, tooltip: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .build();
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

/// The divider identifies dual panes; the toggle state indicates visibility.
pub(super) fn pane_count_toggle_button(tooltip: &str) -> gtk::ToggleButton {
    view_toggle_button("commander-columns-2-symbolic", tooltip)
}

pub(super) fn column_view_toggle_button(tooltip: &str) -> gtk::ToggleButton {
    view_toggle_button("commander-columns-3-symbolic", tooltip)
}
