//! Folder menus for tabs, independent of the file view's current selection.

use super::*;

#[derive(Clone, Debug)]
pub(crate) struct TabFolderTarget {
    pub pane: PaneId,
    pub path: VPath,
}

impl TabFolderTarget {
    pub(in crate::app) fn message(&self, action: AppMsg) -> AppMsg {
        AppMsg::TabFolderAction {
            target: self.clone(),
            action: Box::new(action),
        }
    }

    pub(in crate::app) fn sender(&self, input: relm4::Sender<AppMsg>) -> relm4::Sender<AppMsg> {
        let (sender, receiver) = relm4::channel();
        let target = self.clone();
        // The forwarder ends when the menu or response dialog releases its senders.
        glib::spawn_future_local(receiver.forward(input, move |action| target.message(action)));
        sender
    }
}

pub(in crate::app) fn install_tab_context_menu(
    pill: &gtk::Box,
    parent: &gtk::Box,
    target: TabFolderTarget,
    keymap: Keymap,
    tools: Rc<RefCell<Vec<CustomToolSession>>>,
    input: relm4::Sender<AppMsg>,
    actions: impl Fn() -> action_policy::Context + 'static,
) {
    let weak_pill = pill.downgrade();
    let weak_parent = parent.downgrade();
    let open = Rc::new(move |x: f64, y: f64| {
        let (Some(pill), Some(parent)) = (weak_pill.upgrade(), weak_parent.upgrade()) else {
            return;
        };
        let Some(point) =
            pill.compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
        else {
            return;
        };
        let mut child = parent.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            if let Some(menu) = current.downcast_ref::<gtk::Popover>()
                && menu.has_css_class("file-context-menu")
            {
                menu.popdown();
            }
        }
        let folder = (target.path.clone(), EntryKind::Directory);
        let menu = menu::build_menu(
            parent.upcast_ref(),
            menu::Context {
                pane: target.pane,
                actions: actions(),
            },
            Some(&folder),
            1,
            &keymap,
            &tools.borrow(),
            &target.sender(input.clone()),
        );
        menu.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        menu.popup();
    });
    let gesture = gtk::GestureClick::new();
    gesture.set_button(3);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let clicked = open.clone();
    gesture.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        clicked(x, y);
    });
    pill.add_controller(gesture);
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if key == gdk::Key::Menu
            || (key == gdk::Key::F10 && modifiers == gdk::ModifierType::SHIFT_MASK)
        {
            open(12.0, 24.0);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    pill.add_controller(keys);
}

impl AppModel {
    pub(in crate::app) fn folder_action_input(
        &self,
        sender: &ComponentSender<Self>,
    ) -> relm4::Sender<AppMsg> {
        let input = sender.input_sender().clone();
        self.folder_action_target
            .as_ref()
            .map_or_else(|| input.clone(), |target| target.sender(input.clone()))
    }

    pub(in crate::app) fn on_tab_folder_action(
        &mut self,
        target: TabFolderTarget,
        action: AppMsg,
        sender: &ComponentSender<Self>,
    ) {
        let previous = self.folder_action_target.replace(target.clone());
        self.active_pane = target.pane;
        match action {
            AppMsg::ExecuteCommand(command) => self.execute_command(command, sender),
            AppMsg::RunCustomTool(index) => self.run_custom_tool(index, sender),
            AppMsg::OpenCursor => {
                self.open_path(target.pane, EntryKind::Directory, target.path, sender)
            }
            AppMsg::ToggleQuickLook => {
                self.quick_look_open = true;
                self.start_preview(sender);
            }
            AppMsg::CreateArchive {
                name,
                format,
                password,
            } => self.start_create_archive(name, format, password, sender),
            AppMsg::DeletePermanentConfirmed => {
                self.start_operation(CommandId::DeletePermanent, sender)
            }
            AppMsg::PasteInto(pane, destination) => {
                self.paste_file_clipboard_into(pane, destination, sender)
            }
            _ => tracing::warn!("Ignoring unsupported tab folder action"),
        }
        // A later keyboard/file-view action must use its own selection again.
        // Responses that still need this folder carry the target in their message.
        self.folder_action_target = previous;
    }
}
