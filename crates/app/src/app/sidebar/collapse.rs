use super::*;

pub(in crate::app) struct CollapsibleGroup {
    pub key: String,
    pub heading: gtk::Box,
    pub toggle: gtk::ToggleButton,
    pub content: gtk::Box,
    pub revealer: gtk::Revealer,
    syncing: Rc<Cell<bool>>,
}

impl CollapsibleGroup {
    pub fn new(key: &str, name: &str, expanded: bool, input: &relm4::Sender<AppMsg>) -> Self {
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        heading.add_css_class("sidebar-heading-row");
        let toggle = gtk::ToggleButton::new();
        toggle.add_css_class("flat");
        toggle.add_css_class("sidebar-group-toggle");
        toggle.set_hexpand(true);
        let title = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::new();
        icon.set_pixel_size(12);
        let label = gtk::Label::new(Some(name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(SIDEBAR_LABEL_WIDTH_CHARS);
        title.append(&icon);
        title.append(&label);
        toggle.set_child(Some(&title));
        toggle.update_property(&[gtk::accessible::Property::Label(name)]);
        heading.append(&toggle);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        revealer.set_transition_duration(120);
        revealer.set_child(Some(&content));
        toggle.set_active(expanded);
        let update = {
            let name = name.to_owned();
            let revealer = revealer.clone();
            move |button: &gtk::ToggleButton| {
                let expanded = button.is_active();
                revealer.set_reveal_child(expanded);
                icon.set_icon_name(Some(if expanded {
                    "commander-chevron-down-symbolic"
                } else {
                    "commander-chevron-right-symbolic"
                }));
                button.update_state(&[gtk::accessible::State::Expanded(Some(expanded))]);
                button.set_tooltip_text(Some(&format!(
                    "{} {name}",
                    if expanded { "Collapse" } else { "Expand" }
                )));
            }
        };
        update(&toggle);
        let sender = input.clone();
        let id = key.to_owned();
        let syncing = Rc::new(Cell::new(false));
        let updating = syncing.clone();
        toggle.connect_toggled(move |button| {
            update(button);
            if !updating.get() {
                let _ = sender.send(AppMsg::SetSidebarGroupExpanded {
                    key: id.clone(),
                    expanded: button.is_active(),
                });
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let button = toggle.downgrade();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if modifiers.is_empty()
                && let Some(button) = button.upgrade()
            {
                match key {
                    gdk::Key::Left | gdk::Key::KP_Left => button.set_active(false),
                    gdk::Key::Right | gdk::Key::KP_Right => button.set_active(true),
                    gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => {
                        button.set_active(!button.is_active())
                    }
                    _ => return glib::Propagation::Proceed,
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        toggle.add_controller(keys);
        Self {
            key: key.to_owned(),
            heading,
            toggle,
            content,
            revealer,
            syncing,
        }
    }

    pub fn append_to(&self, parent: &gtk::Box) {
        parent.append(&self.heading);
        parent.append(&self.revealer);
    }

    pub fn set_expanded(&self, expanded: bool) {
        self.syncing.set(true);
        self.toggle.set_active(expanded);
        self.syncing.set(false);
    }
}

pub(in crate::app) fn favorite_group_key(name: &str) -> String {
    format!("favorite:{name}")
}

#[cfg(test)]
mod tests;
