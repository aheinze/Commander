//! A floating alert sheet in the command palette's visual language.
//!
//! Covers the slice of `adw::AlertDialog` the app relies on (responses with an
//! appearance, default and close responses, an optional extra child), but lays
//! the content out left-aligned with a right-aligned button row so alerts read
//! like the utility dialogs instead of a centred system prompt.

use super::*;

struct Response {
    id: String,
    label: String,
    appearance: adw::ResponseAppearance,
}

/// A response handler and the response id it listens for (`None` for all).
type Handler = (Option<String>, Box<dyn Fn(&AlertSheet, &str)>);

#[derive(Clone)]
pub(super) struct AlertSheet {
    dialog: adw::Dialog,
    extra: gtk::Box,
    actions: gtk::Box,
    responses: Rc<RefCell<Vec<Response>>>,
    handlers: Rc<RefCell<Vec<Handler>>>,
    close_response: Rc<RefCell<Option<String>>>,
    default_response: Rc<RefCell<Option<String>>>,
    responded: Rc<Cell<bool>>,
}

impl AlertSheet {
    pub(super) fn new(heading: Option<&str>, body: Option<&str>) -> Self {
        let dialog = adw::Dialog::builder()
            .follows_content_size(true)
            .presentation_mode(adw::DialogPresentationMode::Floating)
            .build();
        dialog.add_css_class("alert-sheet");
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_size_request(400, -1);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.add_css_class("alert-sheet-content");
        let heading_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let heading_space = gtk::Box::new(gtk::Orientation::Vertical, 0);
        heading_space.set_hexpand(true);
        if let Some(heading) = heading {
            let label = gtk::Label::new(Some(heading));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.add_css_class("alert-heading");
            heading_space.append(&label);
        }
        heading_row.append(&heading_space);
        let close = chrome::dialog_close_button(&dialog);
        close.set_valign(gtk::Align::Start);
        heading_row.append(&close);
        content.append(&heading_row);
        if let Some(body) = body {
            let label = gtk::Label::new(Some(body));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            label.set_selectable(true);
            label.set_can_focus(false);
            label.add_css_class("alert-body");
            // Long reports (folder comparisons) scroll; ordinary prompts stay flat so
            // the scroller cannot add slack around a two-line body.
            if body.lines().count() > 12 {
                let scroll = gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .vscrollbar_policy(gtk::PolicyType::Automatic)
                    .propagate_natural_height(true)
                    .max_content_height(340)
                    .child(&label)
                    .build();
                scroll.add_css_class("alert-body-scroll");
                content.append(&scroll);
            } else {
                content.append(&label);
            }
        }
        let extra = gtk::Box::new(gtk::Orientation::Vertical, 0);
        extra.add_css_class("alert-extra");
        extra.set_visible(false);
        content.append(&extra);
        root.append(&content);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("dialog-actions");
        actions.set_halign(gtk::Align::End);
        root.append(&actions);
        dialog.set_child(Some(&root));
        Self {
            dialog,
            extra,
            actions,
            responses: Rc::default(),
            handlers: Rc::default(),
            close_response: Rc::default(),
            default_response: Rc::default(),
            responded: Rc::default(),
        }
    }

    pub(super) fn add_response(&self, id: &str, label: &str) {
        self.responses.borrow_mut().push(Response {
            id: id.to_owned(),
            label: label.to_owned(),
            appearance: adw::ResponseAppearance::Default,
        });
    }

    pub(super) fn set_response_appearance(&self, id: &str, appearance: adw::ResponseAppearance) {
        if let Some(response) = self
            .responses
            .borrow_mut()
            .iter_mut()
            .find(|response| response.id == id)
        {
            response.appearance = appearance;
        }
    }

    pub(super) fn set_close_response(&self, id: &str) {
        *self.close_response.borrow_mut() = Some(id.to_owned());
    }

    pub(super) fn set_default_response(&self, id: Option<&str>) {
        *self.default_response.borrow_mut() = id.map(str::to_owned);
    }

    pub(super) fn set_extra_child(&self, child: Option<&impl IsA<gtk::Widget>>) {
        while let Some(previous) = self.extra.first_child() {
            self.extra.remove(&previous);
        }
        if let Some(child) = child {
            self.extra.append(child);
        }
        self.extra.set_visible(child.is_some());
    }

    /// Registers a handler for one response id, or for every response when
    /// `id` is `None`. Closing the sheet without choosing emits the close response.
    pub(super) fn connect_response<F>(&self, id: Option<&str>, handler: F)
    where
        F: Fn(&Self, &str) + 'static,
    {
        self.handlers
            .borrow_mut()
            .push((id.map(str::to_owned), Box::new(handler)));
    }

    pub(super) fn present(&self, parent: Option<&impl IsA<gtk::Widget>>) {
        let default_button = self.build_actions();
        let sheet = self.clone();
        self.dialog.connect_closed(move |_| {
            if sheet.responded.replace(true) {
                return;
            }
            let close_response = sheet.close_response.borrow().clone();
            if let Some(id) = close_response {
                sheet.emit(&id);
            }
        });
        self.dialog.present(parent);
        // Keyboard focus starts on the default action so Enter and the focus
        // ring agree; callers that want a field focused grab it afterwards.
        if let Some(button) = default_button {
            button.grab_focus();
        }
    }

    fn build_actions(&self) -> Option<gtk::Button> {
        while let Some(previous) = self.actions.first_child() {
            self.actions.remove(&previous);
        }
        let responses = self.responses.borrow();
        let stacked = responses.len() > 3;
        if stacked {
            self.actions.set_orientation(gtk::Orientation::Vertical);
            self.actions.set_halign(gtk::Align::Fill);
            self.actions.add_css_class("dialog-actions-stacked");
        }
        let default_response = self.default_response.borrow().clone();
        let mut default_button = None;
        for response in responses.iter() {
            let button = gtk::Button::with_label(&response.label);
            button.set_hexpand(stacked);
            match response.appearance {
                adw::ResponseAppearance::Suggested => button.add_css_class("suggested-action"),
                adw::ResponseAppearance::Destructive => {
                    button.add_css_class("destructive-action");
                }
                _ => {}
            }
            let sheet = self.clone();
            let id = response.id.clone();
            button.connect_clicked(move |_| {
                sheet.responded.set(true);
                sheet.emit(&id);
                sheet.dialog.close();
            });
            if default_response.as_deref() == Some(response.id.as_str()) {
                self.dialog.set_default_widget(Some(&button));
                default_button = Some(button.clone());
            }
            self.actions.append(&button);
        }
        default_button
    }

    fn emit(&self, id: &str) {
        for (filter, handler) in self.handlers.borrow().iter() {
            if filter.as_deref().is_none_or(|filter| filter == id) {
                handler(self, id);
            }
        }
    }
}
