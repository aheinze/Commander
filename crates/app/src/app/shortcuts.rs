//! Keyboard-shortcut installation and palette key handling.

use super::*;

pub(super) fn connect_button(
    button: &gtk::Button,
    sender: &ComponentSender<AppModel>,
    message: impl Fn() -> AppMsg + 'static,
) {
    let input = sender.input_sender().clone();
    button.connect_clicked(move |_| {
        let _ = input.send(message());
    });
}

pub(super) fn install_shortcuts(
    window: &adw::ApplicationWindow,
    sender: &ComponentSender<AppModel>,
    keymap: Keymap,
    palette: PaletteKeyboardState,
) {
    let PaletteKeyboardState {
        input_active: palette_input_active,
        pending_open: palette_pending_open,
        presented: palette_presented,
        entry: palette_entry,
        dialog: palette_dialog,
    } = palette;
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let input = sender.input_sender().clone();
    let key_window = window.clone();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        if palette_input_active.get() {
            if keymap.command_for(key, modifiers) == Some(CommandId::CommandPalette) {
                palette_input_active.set(false);
                palette_pending_open.set(false);
                let _ = input.send(AppMsg::ExecuteCommand(CommandId::CommandPalette));
                return glib::Propagation::Stop;
            }
            let message = match key {
                gdk::Key::Escape => {
                    palette_input_active.set(false);
                    Some(AppMsg::ClosePalette)
                }
                gdk::Key::BackSpace => Some(AppMsg::SetPaletteQuery(palette_delete_backward(
                    &palette_entry,
                ))),
                gdk::Key::Up | gdk::Key::KP_Up => Some(AppMsg::MovePaletteSelection(-1)),
                gdk::Key::Down | gdk::Key::KP_Down => Some(AppMsg::MovePaletteSelection(1)),
                gdk::Key::Page_Up | gdk::Key::KP_Page_Up => Some(AppMsg::MovePaletteSelection(-7)),
                gdk::Key::Page_Down | gdk::Key::KP_Page_Down => {
                    Some(AppMsg::MovePaletteSelection(7))
                }
                gdk::Key::Home | gdk::Key::KP_Home => {
                    Some(AppMsg::MovePaletteSelectionToEnd(false))
                }
                gdk::Key::End | gdk::Key::KP_End => Some(AppMsg::MovePaletteSelectionToEnd(true)),
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    let _ = input.send(AppMsg::SetPaletteQuery(palette_entry.text().to_string()));
                    Some(AppMsg::ActivatePaletteSelection)
                }
                _ if !modifiers.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK
                        | gdk::ModifierType::META_MASK,
                ) =>
                {
                    key.to_unicode().and_then(|character| {
                        if character.is_control() {
                            return None;
                        }
                        Some(AppMsg::SetPaletteQuery(palette_insert_character(
                            &palette_entry,
                            character,
                        )))
                    })
                }
                _ => None,
            };
            if let Some(message) = message {
                let _ = input.send(message);
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        let focused = gtk::prelude::GtkWindowExt::focus(&key_window);
        if focused.as_ref().is_some_and(|focus| {
            widget_has_ancestor_css_class(
                focus,
                &["file-context-menu", "breadcrumb-ancestors", "tools-dialog"],
            )
        }) {
            // These surfaces own navigation, typing, and activation while focused.
            return glib::Propagation::Proceed;
        }
        if key == gdk::Key::Escape
            && focused
                .as_ref()
                .is_some_and(|focus| widget_has_ancestor_css_class(focus, &["path-entry"]))
        {
            return glib::Propagation::Proceed;
        }
        if !control
            && !alt
            && (matches!(
                key,
                gdk::Key::Return
                    | gdk::Key::KP_Enter
                    | gdk::Key::space
                    | gdk::Key::Left
                    | gdk::Key::Right
                    | gdk::Key::Up
                    | gdk::Key::Down
                    | gdk::Key::Home
                    | gdk::Key::End
            ) || key
                .to_unicode()
                .is_some_and(|character| !character.is_control()))
            && focused.as_ref().is_some_and(|focus| {
                widget_has_ancestor_css_class(focus, &["breadcrumbs", "breadcrumb-icon"])
            })
        {
            return glib::Propagation::Proceed;
        }
        let terminal_focused = focused
            .as_ref()
            .is_some_and(|focus| widget_has_ancestor_css_class(focus, &["terminal-surface"]));
        if terminal_focused {
            if keymap.command_for(key, modifiers) == Some(CommandId::ToggleTerminal) {
                let _ = input.send(AppMsg::ToggleTerminal);
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        let editing_text = focused.as_ref().is_some_and(|focus| {
            focus.is::<gtk::Text>() || focus.is::<gtk::Entry>() || focus.is::<gtk::SearchEntry>()
        });
        let browsing_files = focused.as_ref().is_none_or(|focus| {
            widget_has_ancestor_css_class(focus, &["file-list", "file-grid", "column-browser"])
        });
        let shifted_navigation = if !editing_text && shift && !control && !alt {
            match key {
                gdk::Key::Up | gdk::Key::KP_Up => Some(AppMsg::MoveCursorVertical(-1, true)),
                gdk::Key::Down | gdk::Key::KP_Down => Some(AppMsg::MoveCursorVertical(1, true)),
                gdk::Key::Left | gdk::Key::KP_Left => Some(AppMsg::MoveCursorHorizontal(-1, true)),
                gdk::Key::Right | gdk::Key::KP_Right => Some(AppMsg::MoveCursorHorizontal(1, true)),
                gdk::Key::Home | gdk::Key::KP_Home => {
                    Some(AppMsg::MoveCursorTo(CursorTarget::First, true))
                }
                gdk::Key::End | gdk::Key::KP_End => {
                    Some(AppMsg::MoveCursorTo(CursorTarget::Last, true))
                }
                gdk::Key::Page_Up | gdk::Key::KP_Page_Up => {
                    Some(AppMsg::MoveCursorTo(CursorTarget::PageUp, true))
                }
                gdk::Key::Page_Down | gdk::Key::KP_Page_Down => {
                    Some(AppMsg::MoveCursorTo(CursorTarget::PageDown, true))
                }
                _ => None,
            }
        } else {
            None
        };
        let message = if shifted_navigation.is_some() {
            shifted_navigation
        } else if let Some(command) = keymap.command_for(key, modifiers)
            && (!editing_text
                || matches!(command, CommandId::ClearLayered | CommandId::CommandPalette))
            && (command != CommandId::SwitchPane || browsing_files)
        {
            Some(AppMsg::ExecuteCommand(command))
        } else if !editing_text && !control && !alt {
            key.to_unicode()
                .filter(|character| !character.is_control())
                .map(AppMsg::AppendFilter)
        } else {
            None
        };
        if let Some(message) = message {
            if matches!(message, AppMsg::ExecuteCommand(CommandId::CommandPalette)) {
                palette_input_active.set(true);
                palette_pending_open.set(true);
                if !palette_presented.replace(true) {
                    palette_dialog.present(Some(&key_window));
                }
                palette_entry.grab_focus();
            }
            let _ = input.send(message);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(controller);
}

pub(super) fn palette_insert_character(entry: &gtk::SearchEntry, character: char) -> String {
    let mut position = entry.position();
    if let Some((start, end)) = entry.selection_bounds() {
        entry.delete_text(start, end);
        position = start;
    }
    entry.insert_text(&character.to_string(), &mut position);
    entry.set_position(position);
    entry.text().to_string()
}

pub(super) fn palette_delete_backward(entry: &gtk::SearchEntry) -> String {
    if entry.selection_bounds().is_some() {
        entry.delete_selection();
    } else {
        let position = entry.position();
        if position > 0 {
            entry.delete_text(position - 1, position);
            entry.set_position(position - 1);
        }
    }
    entry.text().to_string()
}

pub(super) fn widget_has_ancestor_css_class(widget: &gtk::Widget, classes: &[&str]) -> bool {
    let mut current = Some(widget.clone());
    while let Some(widget) = current {
        if classes.iter().any(|class| widget.has_css_class(class)) {
            return true;
        }
        current = widget.parent();
    }
    false
}
