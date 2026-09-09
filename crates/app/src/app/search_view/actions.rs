use super::*;

#[derive(Clone)]
struct Context {
    pane: PaneId,
    destination: VPath,
    policy: action_policy::Context,
}

pub(in crate::app) struct Controls {
    pub selection: gtk::MultiSelection,
    pub store: gio::ListStore,
    input: relm4::Sender<AppMsg>,
    keymap: Keymap,
    context: RefCell<Context>,
    pub count: gtk::Label,
    pub reveal: gtk::Button,
    pub copy: gtk::Button,
    pub more: gtk::Button,
    popup: RefCell<Option<gtk::Popover>>,
}

impl Controls {
    pub fn new(input: relm4::Sender<AppMsg>, keymap: Keymap) -> Rc<Self> {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let count = gtk::Label::new(Some("No selection"));
        count.set_xalign(0.0);
        count.set_hexpand(true);
        count.add_css_class("search-selection-count");
        let controls = Rc::new(Self {
            selection,
            store,
            input,
            keymap,
            context: RefCell::new(Context {
                pane: PaneId::Left,
                destination: VPath::from("/"),
                policy: action_policy::Context::default(),
            }),
            count,
            reveal: gtk::Button::with_label("Show in folder"),
            copy: gtk::Button::with_label("Copy"),
            more: gtk::Button::with_label("Actions"),
            popup: RefCell::new(None),
        });
        let weak = Rc::downgrade(&controls);
        controls
            .selection
            .connect_selection_changed(move |_, _, _| {
                if let Some(controls) = weak.upgrade() {
                    controls.sync();
                }
            });
        for (button, command) in [
            (&controls.reveal, CommandId::Reveal),
            (&controls.copy, CommandId::CopyClipboard),
        ] {
            let weak = Rc::downgrade(&controls);
            button.connect_clicked(move |_| {
                if let Some(controls) = weak.upgrade() {
                    controls.send(command);
                }
            });
        }
        let weak = Rc::downgrade(&controls);
        controls.more.connect_clicked(move |button| {
            if let Some(controls) = weak.upgrade() {
                controls.menu(button.upcast_ref(), None);
            }
        });
        controls.sync();
        controls
    }

    pub fn hits(&self) -> Vec<SearchHit> {
        let selected = self.selection.selection();
        (0..selected.size())
            .filter_map(|index| self.hit(selected.nth(index as u32)))
            .collect()
    }

    pub fn hit(&self, position: u32) -> Option<SearchHit> {
        self.store
            .item(position)
            .and_downcast::<glib::BoxedAnyObject>()
            .map(|item| item.borrow::<SearchHit>().clone())
    }

    fn request(&self, command: CommandId, hits: Vec<SearchHit>) -> search_actions::Request {
        let context = self.context.borrow();
        search_actions::Request {
            command,
            hits,
            pane: context.pane,
            destination: context.destination.clone(),
        }
    }

    pub fn send(&self, command: CommandId) {
        let _ = self
            .input
            .send(AppMsg::SearchAction(self.request(command, self.hits())));
    }

    pub fn activate(&self, position: u32) {
        if let Some(hit) = self.hit(position) {
            let _ = self.input.send(AppMsg::SearchAction(
                self.request(CommandId::Open, vec![hit]),
            ));
        }
    }

    fn reason(&self, command: CommandId, hits: &[SearchHit]) -> Option<&'static str> {
        search_actions::selection_reason(command, hits).or_else(|| {
            let mut policy = self.context.borrow().policy;
            policy.items = hits.len();
            policy.action(command).reason
        })
    }

    fn sync(&self) {
        let hits = self.hits();
        self.count.set_label(&format!("{} selected", hits.len()));
        for (button, command, tooltip) in [
            (
                &self.reveal,
                CommandId::Reveal,
                "Show the selected result in its containing folder · Alt+Enter",
            ),
            (
                &self.copy,
                CommandId::CopyClipboard,
                "Copy the selected results to the clipboard",
            ),
        ] {
            let reason = self.reason(command, &hits);
            button.set_sensitive(reason.is_none());
            button.set_tooltip_text(Some(reason.unwrap_or(tooltip)));
        }
        self.more.set_sensitive(!hits.is_empty());
        self.more
            .set_tooltip_text(Some("Actions for the selected results · Shift+F10"));
    }

    pub fn update(&self, model: &AppModel) {
        let pane = model
            .search_session
            .as_ref()
            .map_or(model.active_pane, |s| s.pane);
        let destination = model.pane(pane.other()).current_directory().clone();
        let mut policy = model.action_context(pane);
        if let Some(hit) = model.search_results.hits.first() {
            policy.location = model.location_policy(&hit.path);
        }
        policy.destination = model.location_policy(&destination);
        *self.context.borrow_mut() = Context {
            pane,
            destination,
            policy,
        };
        self.sync();
    }

    /// Replace a completed search snapshot without selecting a different file
    /// when rows disappear or reorder. The list remains virtualized.
    pub fn replace(&self, hits: &[SearchHit]) {
        let selected: HashSet<_> = self.hits().into_iter().map(|hit| hit.path).collect();
        let items: Vec<_> = hits
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        self.store.splice(0, self.store.n_items(), &items);
        let selection = gtk::Bitset::new_empty();
        for (position, hit) in hits.iter().enumerate() {
            if selected.contains(&hit.path) {
                selection.add(position as u32);
            }
        }
        self.selection
            .set_selection(&selection, &gtk::Bitset::new_range(0, self.store.n_items()));
        self.sync();
    }

    pub fn close_menu(&self) {
        if let Some(menu) = self.popup.borrow_mut().take() {
            menu.popdown();
        }
    }

    pub fn menu(self: &Rc<Self>, anchor: &gtk::Widget, point: Option<(f64, f64)>) {
        self.close_menu();
        let hits = self.hits();
        if hits.is_empty() {
            return;
        }
        let (menu, content) = context_menu::context_action_menu();
        menu.add_css_class("search-result-menu");
        menu.set_parent(anchor);
        if let Some((x, y)) = point {
            menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        }
        let title = gtk::Label::new(Some(&if hits.len() == 1 {
            hits[0]
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            format!("{} selected results", hits.len())
        }));
        title.add_css_class("context-menu-title");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        title.set_max_width_chars(32);
        content.append(&title);
        use CommandId::*;
        let mut groups = Vec::new();
        for commands in [
            vec![
                (Open, "Open", "commander-folder-open-symbolic"),
                (Reveal, "Show in folder", "commander-folder-symbolic"),
                (
                    OpenInNewTab,
                    "Open in new tab",
                    "commander-square-plus-symbolic",
                ),
                (
                    OpenOtherPane,
                    "Open in other pane",
                    "commander-columns-2-symbolic",
                ),
                (OpenWith, "Open with…", "commander-app-window-symbolic"),
            ],
            vec![
                (Cut, "Cut", "commander-scissors-symbolic"),
                (Paste, "Paste into folder", "commander-clipboard-symbolic"),
                (CopyClipboard, "Copy", "commander-copy-symbolic"),
                (CopyPath, "Copy path", "commander-copy-symbolic"),
            ],
            vec![
                (Copy, "Copy to other pane", "commander-copy-symbolic"),
                (
                    Move,
                    "Move to other pane",
                    "commander-corner-up-right-symbolic",
                ),
                (
                    if hits.len() == 1 { Rename } else { BatchRename },
                    "Rename…",
                    "commander-file-pen-line-symbolic",
                ),
            ],
            vec![
                (Trash, "Move to Trash", "commander-trash-symbolic"),
                (
                    DeletePermanent,
                    "Delete permanently…",
                    "commander-trash-symbolic",
                ),
            ],
        ] {
            let mut rows = Vec::new();
            for (command, label, icon) in commands {
                let mut policy = self.context.borrow().policy;
                policy.items = hits.len();
                let decision = policy.action(command);
                if !decision.visible || search_actions::selection_reason(command, &hits).is_some() {
                    continue;
                }
                let shortcut = if command == Reveal {
                    "Alt+Enter".into()
                } else {
                    self.keymap.binding_label(command)
                };
                let label = decision.label(label);
                let button = context_menu::context_menu_item_button(label, icon, Some(&shortcut));
                button.set_sensitive(decision.enabled());
                if let Some(reason) = decision.reason {
                    button.set_tooltip_text(Some(reason));
                }
                if matches!(command, Copy | Move) {
                    button.set_tooltip_text(Some(&format!(
                        "{label}: {}",
                        self.context.borrow().destination
                    )));
                }
                if matches!(command, Trash | DeletePermanent) {
                    button.add_css_class("destructive-action");
                }
                // Capture both source paths and destination when opening the menu.
                let request = self.request(command, hits.clone());
                let input = self.input.clone();
                let weak_menu = menu.downgrade();
                button.connect_clicked(move |_| {
                    let _ = input.send(AppMsg::SearchAction(request.clone()));
                    if let Some(menu) = weak_menu.upgrade() {
                        menu.popdown();
                    }
                });
                rows.push((button, label.to_owned()));
            }
            context_menu::append_context_menu_group(&content, &mut groups, rows);
        }
        let buttons: Vec<_> = groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .map(|(button, _)| button.clone())
            .filter(|button| button.is_sensitive())
            .collect();
        let first = buttons.first().map(|button| button.downgrade());
        menu.connect_map(move |_| {
            if let Some(button) = first.as_ref().and_then(|button| button.upgrade()) {
                button.grab_focus();
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, _| {
            if buttons.is_empty() {
                return glib::Propagation::Proceed;
            }
            let current = buttons
                .iter()
                .position(|button| button.has_focus())
                .unwrap_or(0);
            let next = match key {
                gdk::Key::Down | gdk::Key::KP_Down => (current + 1) % buttons.len(),
                gdk::Key::Up | gdk::Key::KP_Up => (current + buttons.len() - 1) % buttons.len(),
                gdk::Key::Home => 0,
                gdk::Key::End => buttons.len() - 1,
                _ => return glib::Propagation::Proceed,
            };
            buttons[next].grab_focus();
            glib::Propagation::Stop
        });
        menu.add_controller(keys);
        menu.connect_closed(|menu| menu.unparent());
        self.popup.replace(Some(menu.clone()));
        menu.popup();
    }

    pub fn install_keys(self: &Rc<Self>, list: &gtk::ListView) {
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        let weak_list = list.downgrade();
        controller.connect_key_pressed(move |_, key, _, modifiers| {
            let (Some(controls), Some(list)) = (weak.upgrade(), weak_list.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            if key == gdk::Key::Menu
                || (key == gdk::Key::F10 && modifiers == gdk::ModifierType::SHIFT_MASK)
            {
                controls.menu(
                    list.upcast_ref(),
                    Some((f64::from(list.width()) / 2.0, 30.0)),
                );
                return glib::Propagation::Stop;
            }
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
                && modifiers == gdk::ModifierType::ALT_MASK
            {
                controls.send(CommandId::Reveal);
                return glib::Propagation::Stop;
            }
            let Some(command) = controls.keymap.command_for(key, modifiers) else {
                return glib::Propagation::Proceed;
            };
            match command {
                CommandId::SelectAll => {
                    controls.selection.select_all();
                }
                CommandId::ClearSelection => {
                    controls.selection.unselect_all();
                }
                CommandId::InvertSelection => {
                    let all = gtk::Bitset::new_range(0, controls.store.n_items());
                    let inverse = all.copy();
                    inverse.subtract(&controls.selection.selection());
                    controls.selection.set_selection(&inverse, &all);
                }
                CommandId::Refresh => {
                    let _ = controls.input.send(AppMsg::RefreshSearch);
                }
                CommandId::Undo | CommandId::Redo => {
                    let _ = controls.input.send(AppMsg::ExecuteCommand(command));
                }
                // Let GTK activate the focused row with Enter, even if no rows
                // are selected, and own arrows/Space/range selection.
                CommandId::Open if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) => {
                    return glib::Propagation::Proceed;
                }
                command if search_actions::supported(command) => controls.send(command),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        list.add_controller(controller);
    }
}
