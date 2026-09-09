//! Searchable right-click menus and the toolbar buttons that share their vocabulary.

use super::*;

mod menu;
mod tabs;
pub(super) use tabs::{TabFolderTarget, install_tab_context_menu};
#[cfg(test)]
mod tests;

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
    pane_state: Rc<RefCell<PaneDragState>>,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(3);
    let weak_widget = widget.as_ref().downgrade();
    let input = sender.input_sender().clone();
    gesture.connect_pressed(move |gesture, _, x, y| {
        let Some(widget) = weak_widget.upgrade() else {
            return;
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        let target = context_target_at(&widget, x, y);
        let selected = target.as_ref().map_or(0, |(path, _)| {
            let state = pane_state.borrow();
            if state.selected.contains(path) {
                state.selected.len()
            } else {
                1
            }
        });
        let _ = input.send(AppMsg::ContextTarget(pane, target.clone()));
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            if let Some(menu) = current.downcast_ref::<gtk::Popover>()
                && menu.has_css_class("file-context-menu")
            {
                menu.popdown();
            }
        }
        let popover = menu::build_menu(
            &widget,
            menu::Context {
                pane,
                actions: pane_state.borrow().actions,
            },
            target.as_ref(),
            selected,
            &keymap,
            &custom_tools.borrow(),
            &input,
        );
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.popup();
    });
    widget.as_ref().add_controller(gesture);
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
    let query = query.trim().to_lowercase();
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
    let popover = popover.downgrade();
    button.connect_clicked(move |_| {
        let _ = input.send(AppMsg::ExecuteCommand(command));
        if let Some(popover) = popover.upgrade() {
            popover.popdown();
        }
    });
    (
        button,
        format!("{label} {shortcut} {}", command.as_str()).to_lowercase(),
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
    let popover = popover.downgrade();
    button.connect_clicked(move |_| {
        let _ = input.send(AppMsg::RunCustomTool(index));
        if let Some(popover) = popover.upgrade() {
            popover.popdown();
        }
    });
    (button, format!("{label} custom tool").to_lowercase())
}

/// A compact action menu using the same container and row styling as file menus.
pub(super) fn context_action_menu() -> (gtk::Popover, gtk::Box) {
    let menu = gtk::Popover::new();
    menu.add_css_class("file-context-menu");
    menu.set_autohide(true);
    menu.set_has_arrow(false);
    let actions = gtk::Box::new(gtk::Orientation::Vertical, 0);
    actions.set_margin_top(6);
    actions.set_margin_bottom(6);
    actions.set_margin_start(6);
    actions.set_margin_end(6);
    menu.set_child(Some(&actions));
    (menu, actions)
}

/// Builds the shared full-width action row used by application context menus.
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
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.set_max_width_chars(30);
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button.set_tooltip_text(Some(label));
    content.append(&image);
    content.append(&text);
    if let Some(shortcut) = shortcut {
        let shortcut_label = gtk::Label::new(Some(shortcut));
        shortcut_label.add_css_class("context-menu-shortcut");
        shortcut_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        shortcut_label.set_max_width_chars(16);
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
        button.set_tooltip_text(Some(&format!("Set {color} tag")));
        button.update_property(&[gtk::accessible::Property::Label(&format!(
            "Set {color} tag"
        ))]);
        let input = input.clone();
        let popover = popover.downgrade();
        button.connect_clicked(move |_| {
            let _ = input.send(AppMsg::ExecuteCommand(command));
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
        });
        tags.append(&button);
    }
    let clear = icon_button("commander-x-symbolic", "Clear tag");
    clear.add_css_class("flat");
    clear.add_css_class("context-tag-clear");
    clear.set_tooltip_text(Some("Clear tag"));
    let input = input.clone();
    let popover = popover.downgrade();
    clear.connect_clicked(move |_| {
        let _ = input.send(AppMsg::ExecuteCommand(CommandId::ClearTag));
        if let Some(popover) = popover.upgrade() {
            popover.popdown();
        }
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
