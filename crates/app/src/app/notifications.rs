//! Shared, bounded feedback for the main window and its currently visible sheet.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
    Info,
    Success,
    Error,
}

#[derive(Default)]
struct Bus {
    recent: VecDeque<(Kind, String, Instant, glib::WeakRef<adw::ToastOverlay>)>,
    pending: VecDeque<glib::WeakRef<adw::Toast>>,
    observed: VecDeque<(String, String)>,
}

thread_local! { static BUS: RefCell<Bus> = RefCell::default(); }

pub(super) fn wrap(child: &impl IsA<gtk::Widget>) -> adw::ToastOverlay {
    let overlay = adw::ToastOverlay::new();
    overlay.add_css_class("commander-notifications");
    overlay.set_child(Some(child));
    overlay
}

fn active_overlay(in_dialog: bool) -> Option<adw::ToastOverlay> {
    let window = relm4::main_application()
        .active_window()?
        .downcast::<adw::ApplicationWindow>()
        .ok()?;
    if in_dialog && let Some(dialog) = window.visible_dialog() {
        let child = dialog.child()?;
        if let Ok(overlay) = child.clone().downcast::<adw::ToastOverlay>() {
            return Some(overlay);
        }
        // Custom sheets share the same feedback surface as utility dialogs.
        dialog.set_child(gtk::Widget::NONE);
        let overlay = wrap(&child);
        dialog.set_child(Some(&overlay));
        return Some(overlay);
    }
    window.content()?.downcast().ok()
}

pub(super) fn show(kind: Kind, message: &str) {
    make(kind, message, true, true);
}

pub(super) fn action(
    kind: Kind,
    message: &str,
    label: &str,
    callback: impl Fn() + 'static,
) -> Option<adw::Toast> {
    let toast = make(kind, message, true, false)?;
    toast.set_button_label(Some(label));
    toast.set_timeout(10);
    toast.connect_button_clicked(move |_| callback());
    Some(toast)
}

fn make(kind: Kind, message: &str, in_dialog: bool, copy_long: bool) -> Option<adw::Toast> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let overlay = active_overlay(in_dialog)?;
    let duplicate = BUS.with_borrow_mut(|bus| {
        let now = Instant::now();
        bus.recent.retain(|(_, _, at, surface)| {
            surface.upgrade().is_some() && now.duration_since(*at) < Duration::from_secs(2)
        });
        if bus.recent.iter().any(|(level, text, _, surface)| {
            *level == kind && text == message && surface.upgrade().as_ref() == Some(&overlay)
        }) {
            return true;
        }
        bus.recent
            .push_back((kind, message.to_owned(), now, overlay.downgrade()));
        while bus.recent.len() > 32 {
            bus.recent.pop_front();
        }
        false
    });
    if duplicate {
        return None;
    }
    let toast = adw::Toast::new(message);
    toast.set_use_markup(false);
    toast.set_timeout(if kind == Kind::Error { 10 } else { 5 });
    if kind == Kind::Error {
        toast.set_priority(adw::ToastPriority::High);
    }
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.add_css_class("notification-content");
    let (icon, class, label) = match kind {
        Kind::Info => (
            "commander-info-symbolic",
            "notification-info",
            "Information",
        ),
        Kind::Success => (
            "commander-circle-check-symbolic",
            "notification-success",
            "Completed",
        ),
        Kind::Error => (
            "commander-triangle-alert-symbolic",
            "notification-error",
            "Error",
        ),
    };
    content.add_css_class(class);
    let icon = gtk::Image::from_icon_name(icon);
    icon.set_pixel_size(18);
    content.append(&icon);
    let text = gtk::Label::new(Some(message));
    text.set_xalign(0.0);
    text.set_wrap(true);
    text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.set_lines(3);
    text.set_max_width_chars(58);
    text.set_width_chars(1);
    text.set_hexpand(true);
    text.set_tooltip_text(Some(message));
    text.update_property(&[gtk::accessible::Property::Label(&format!(
        "{label}: {message}"
    ))]);
    content.append(&text);
    toast.set_custom_title(Some(&content));
    // Even short diagnostics can truncate in compact sheets (e.g. wide Unicode).
    if copy_long && (kind == Kind::Error || message.chars().count() > 160 || message.contains('\n'))
    {
        toast.set_button_label(Some("Copy"));
        let message = message.to_owned();
        toast.connect_button_clicked(move |_| {
            if let Some(display) = gdk::Display::default() {
                display.clipboard().set_text(&message);
            }
        });
    }
    let (admitted, expired) = BUS.with_borrow_mut(|bus| {
        bus.pending.retain(|toast| toast.upgrade().is_some());
        let expired = if bus.pending.len() >= 4 {
            let lower_priority = bus.pending.iter().position(|item| {
                item.upgrade()
                    .is_some_and(|toast| toast.priority() != adw::ToastPriority::High)
            });
            let Some(index) = lower_priority.or((kind == Kind::Error).then_some(0)) else {
                return (false, None);
            };
            bus.pending.remove(index).and_then(|toast| toast.upgrade())
        } else {
            None
        };
        bus.pending.push_back(toast.downgrade());
        (true, expired)
    });
    if !admitted {
        return None;
    }
    if let Some(expired) = expired {
        expired.dismiss();
    }
    toast.connect_dismissed(|dismissed| {
        BUS.with_borrow_mut(|bus| {
            bus.pending
                .retain(|item| item.upgrade().is_some_and(|toast| toast != *dismissed))
        })
    });
    overlay.add_toast(toast.clone());
    Some(toast)
}

pub(super) fn error(message: &str) {
    show(Kind::Error, message);
}
pub(super) fn info(message: &str) {
    show(Kind::Info, message);
}
pub(super) fn success(message: &str) {
    make(Kind::Success, message, false, true);
}

/// Observes model state without replaying errors on redraw, resize, or progress ticks.
pub(super) fn observe(source: impl Into<String>, message: Option<&str>, kind: Kind) {
    if message.is_some() && relm4::main_application().active_window().is_none() {
        return;
    }
    let source = source.into();
    let changed = BUS.with_borrow_mut(|bus| {
        let previous = bus
            .observed
            .iter()
            .position(|(key, _)| key == &source)
            .and_then(|index| bus.observed.remove(index));
        let Some(message) = message.filter(|message| !message.is_empty()) else {
            return false;
        };
        let changed = previous.as_ref().is_none_or(|(_, old)| old != message);
        bus.observed.push_back((source, message.to_owned()));
        while bus.observed.len() > 128 {
            bus.observed.pop_front();
        }
        changed
    });
    if changed && let Some(message) = message {
        show(kind, message);
    }
}

/// A sheet's feedback state. It has no inline widget and suppresses unchanged messages.
#[derive(Clone, Default)]
pub(super) struct Feedback(Rc<RefCell<Option<(Kind, String)>>>);

impl Feedback {
    pub fn clear(&self) {
        self.0.borrow_mut().take();
    }
    fn send(&self, kind: Kind, message: &str) {
        if message.is_empty() {
            self.clear();
            return;
        }
        let next = (kind, message.to_owned());
        if self.0.borrow().as_ref() == Some(&next) {
            return;
        }
        self.0.replace(Some(next));
        show(kind, message);
    }
    pub fn info(&self, message: &str) {
        self.send(Kind::Info, message);
    }
    pub fn error(&self, message: &str) {
        self.send(Kind::Error, message);
    }
    pub fn success(&self, message: &str) {
        self.send(Kind::Success, message);
    }
    pub fn validate(&self, message: Option<&str>, field: &impl IsA<gtk::Widget>) {
        // Keep a field-associated way to recover dismissed validation without
        // reserving inline space or announcing it again on every keystroke.
        field.set_tooltip_text(message);
        field
            .as_ref()
            .update_property(&[gtk::accessible::Property::Description(
                message.unwrap_or_default(),
            )]);
        let Some(message) = message else {
            self.clear();
            return;
        };
        let next = (Kind::Error, message.to_owned());
        if self.0.borrow().as_ref() == Some(&next) {
            return;
        }
        self.0.replace(Some(next.clone()));
        let state = Rc::downgrade(&self.0);
        let field = field.as_ref().downgrade();
        glib::timeout_add_local_once(Duration::from_millis(650), move || {
            if field.upgrade().is_some_and(|field| field.is_mapped())
                && state
                    .upgrade()
                    .is_some_and(|state| state.borrow().as_ref() == Some(&next))
            {
                error(&next.1);
            }
        });
    }
    #[cfg(test)]
    pub fn text(&self) -> String {
        self.0
            .borrow()
            .as_ref()
            .map(|(_, text)| text.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
pub(super) fn test_messages() -> Vec<String> {
    BUS.with_borrow(|bus| {
        bus.pending
            .iter()
            .filter_map(|item| {
                item.upgrade()
                    .and_then(|toast| toast.custom_title())
                    .and_then(|title| title.last_child())
                    .and_downcast::<gtk::Label>()
                    .map(|label| label.text().to_string())
            })
            .collect()
    })
}

#[cfg(test)]
mod tests;
