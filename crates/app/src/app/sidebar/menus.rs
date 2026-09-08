//! Sidebar menus share file-menu rows and operate on their captured location.

use super::super::*;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
pub(crate) enum LocationAction {
    Open,
    NewTab,
    OtherPane,
    Terminal,
    CopyPath,
    Bookmark,
}

pub(in crate::app) fn new(parent: &impl IsA<gtk::Widget>, name: &str) -> (gtk::Popover, gtk::Box) {
    let (menu, actions) = context_menu::context_action_menu();
    menu.add_css_class("sidebar-context-menu");
    let title = gtk::Label::new(Some(name));
    title.add_css_class("context-menu-title");
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    title.set_max_width_chars(28);
    title.set_single_line_mode(true);
    title.set_tooltip_text(Some(name));
    actions.append(&title);
    separator(&actions);
    menu.set_parent(parent);
    let weak = menu.downgrade();
    parent.connect_destroy(move |_| {
        if let Some(menu) = weak.upgrade() {
            menu.set_child(gtk::Widget::NONE);
            if menu.parent().is_some() {
                menu.unparent();
            }
        }
    });
    menu.connect_map(|menu| {
        if let Some(first) = buttons(menu.upcast_ref()).first() {
            first.grab_focus();
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = menu.downgrade();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        let Some(menu) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if key == gdk::Key::Escape {
            menu.popdown();
            return glib::Propagation::Stop;
        }
        if !modifiers.is_empty() {
            return glib::Propagation::Proceed;
        }
        let rows = buttons(menu.upcast_ref());
        if rows.is_empty() {
            return glib::Propagation::Proceed;
        }
        let focus = menu.root().and_then(|root| root.focus());
        let current = rows
            .iter()
            .position(|row| {
                focus.as_ref().is_some_and(|focus| {
                    focus == row.upcast_ref::<gtk::Widget>() || focus.is_ancestor(row)
                })
            })
            .unwrap_or(0);
        let index = match key {
            gdk::Key::Down | gdk::Key::KP_Down => (current + 1) % rows.len(),
            gdk::Key::Up | gdk::Key::KP_Up => (current + rows.len() - 1) % rows.len(),
            gdk::Key::Home => 0,
            gdk::Key::End => rows.len() - 1,
            _ => return glib::Propagation::Proceed,
        };
        rows[index].grab_focus();
        glib::Propagation::Stop
    });
    menu.add_controller(keys);
    (menu, actions)
}

fn buttons(root: &gtk::Widget) -> Vec<gtk::Button> {
    if let Some(button) = root.downcast_ref::<gtk::Button>()
        && button.has_css_class("context-menu-item")
    {
        return if button.is_sensitive() && button.is_visible() {
            vec![button.clone()]
        } else {
            vec![]
        };
    }
    let mut result = Vec::new();
    let mut child = root.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        result.extend(buttons(&widget));
    }
    result
}

pub(in crate::app) fn separator(actions: &gtk::Box) {
    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    separator.add_css_class("context-menu-separator");
    actions.append(&separator);
}

pub(in crate::app) fn action(
    menu: &gtk::Popover,
    actions: &gtk::Box,
    label: &str,
    icon: &str,
    run: impl Fn() + 'static,
) -> gtk::Button {
    let button = context_menu_item_button(label, icon, None);
    let weak = menu.downgrade();
    button.connect_clicked(move |_| {
        if let Some(menu) = weak.upgrade() {
            menu.popdown();
        }
        run();
    });
    actions.append(&button);
    button
}

pub(in crate::app) fn message(
    menu: &gtk::Popover,
    actions: &gtk::Box,
    label: &str,
    icon: &str,
    input: &relm4::Sender<AppMsg>,
    message: impl Fn() -> AppMsg + 'static,
) -> gtk::Button {
    let input = input.clone();
    action(menu, actions, label, icon, move || {
        let _ = input.send(message());
    })
}

pub(in crate::app) fn location(
    parent: &impl IsA<gtk::Widget>,
    name: &str,
    path: &VPath,
    bookmark: bool,
    input: &relm4::Sender<AppMsg>,
) {
    let (menu, actions) = new(parent, name);
    location_actions(&menu, &actions, path, bookmark, input);
    install(parent, &menu);
}

pub(in crate::app) fn location_actions(
    menu: &gtk::Popover,
    actions: &gtk::Box,
    path: &VPath,
    bookmark: bool,
    input: &relm4::Sender<AppMsg>,
) {
    for (label, icon, action) in [
        (
            "Open",
            "commander-folder-open-symbolic",
            LocationAction::Open,
        ),
        (
            "Open in new tab",
            "commander-square-plus-symbolic",
            LocationAction::NewTab,
        ),
        (
            "Open in other pane",
            "commander-panel-right-symbolic",
            LocationAction::OtherPane,
        ),
        (
            "Open in terminal",
            "commander-terminal-symbolic",
            LocationAction::Terminal,
        ),
    ] {
        let path = path.clone();
        message(menu, actions, label, icon, input, move || {
            AppMsg::SidebarLocation {
                path: path.clone(),
                action,
            }
        });
    }
    separator(actions);
    let target = path.clone();
    message(
        menu,
        actions,
        "Copy path",
        "commander-copy-symbolic",
        input,
        move || AppMsg::SidebarLocation {
            path: target.clone(),
            action: LocationAction::CopyPath,
        },
    );
    if bookmark {
        let path = path.clone();
        message(
            menu,
            actions,
            "Add to Favorites",
            "commander-folder-plus-symbolic",
            input,
            move || AppMsg::SidebarLocation {
                path: path.clone(),
                action: LocationAction::Bookmark,
            },
        );
    }
}

/// One pointer/keyboard contract for all sidebar entry types, including group headings.
pub(in crate::app) fn install(trigger: &impl IsA<gtk::Widget>, menu: &gtk::Popover) {
    let weak_trigger = trigger.as_ref().downgrade();
    let menu = menu.clone();
    let open = Rc::new(move |point: Option<(f64, f64)>| {
        let Some(trigger) = weak_trigger.upgrade() else {
            return;
        };
        let rectangle = point.and_then(|(x, y)| {
            let parent = menu.parent()?;
            let point =
                trigger.compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))?;
            Some(gdk::Rectangle::new(
                point.x() as i32,
                point.y() as i32,
                1,
                1,
            ))
        });
        menu.set_pointing_to(rectangle.as_ref());
        menu.popup();
    });
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let clicked = open.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        clicked(Some((x, y)));
    });
    trigger.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if key == gdk::Key::Menu
            || (key == gdk::Key::F10 && modifiers == gdk::ModifierType::SHIFT_MASK)
        {
            open(None);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    trigger.add_controller(keys);
}

impl AppModel {
    pub(in crate::app) fn sidebar_location(
        &mut self,
        path: VPath,
        action: LocationAction,
        sender: &ComponentSender<Self>,
    ) {
        match action {
            LocationAction::Open => self.navigate(self.active_pane, path, sender),
            LocationAction::Terminal => {
                self.terminal_visible = true;
                self.start_terminal_at(path, sender);
            }
            LocationAction::Bookmark => {
                if !self.bookmarks.contains(&path) {
                    self.bookmarks.push(path);
                    self.persist_session();
                }
            }
            LocationAction::NewTab | LocationAction::OtherPane | LocationAction::CopyPath => {
                let command = match action {
                    LocationAction::NewTab => CommandId::OpenInNewTab,
                    LocationAction::OtherPane => CommandId::OpenOtherPane,
                    _ => CommandId::CopyPath,
                };
                self.on_tab_folder_action(
                    context_menu::TabFolderTarget {
                        pane: self.active_pane,
                        path,
                    },
                    AppMsg::ExecuteCommand(command),
                    sender,
                );
            }
        }
    }
}
