//! Password sheets bridge archive workers to the main thread without blocking GTK.
use super::*;
use crate::archive::Password;
use std::sync::mpsc::{RecvTimeoutError, SyncSender};

#[derive(Clone, Copy, Debug)]
pub(super) enum Context {
    Browse(PaneId, JobId),
    Reload(JobId),
    Operation(JobId),
}

#[derive(Debug)]
pub(crate) struct Request {
    context: Context,
    source: VPath,
    incorrect: bool,
    cancel: CancelToken,
    reply: SyncSender<Option<Password>>,
}

// Only called on an archive worker. Cancellation remains responsive while a
// sheet is open, and the generation check below rejects stale worker requests.
pub(super) fn request(
    input: &relm4::Sender<AppMsg>,
    context: Context,
    source: &VPath,
    incorrect: bool,
    cancel: &CancelToken,
) -> Option<Password> {
    let (reply, receiver) = std::sync::mpsc::sync_channel(1);
    input
        .send(AppMsg::ArchivePasswordRequested(Request {
            context,
            source: source.clone(),
            incorrect,
            cancel: cancel.clone(),
            reply,
        }))
        .ok()?;
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(password) => return password,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

impl AppModel {
    pub(super) fn on_archive_password_request(&mut self, request: Request) {
        let current = match request.context {
            Context::Browse(pane, id) => self.archive_browse_is_current(pane, id),
            Context::Operation(id) => self
                .operations
                .get(&id)
                .is_some_and(OperationStatus::is_active),
            Context::Reload(id) => self.archive_reload_is_current(&request.source, id),
        };
        if !current || request.cancel.is_cancelled() {
            return;
        }
        let source = self.archive_mounts.display(&request.source);
        show_password_dialog(source, request);
    }
}

fn show_password_dialog(source: VPath, request: Request) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let (dialog, view) =
        dialogs::utility_dialog("Unlock Archive", 440, 0, "archive-password-dialog");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.add_css_class("dialog-body");
    let description = gtk::Label::new(Some(&format!("Enter the password for {source}.")));
    description.set_xalign(0.0);
    description.set_wrap(true);
    description.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    body.append(&description);
    if request.incorrect {
        let error = gtk::Label::new(Some(
            "The password was not accepted, or the archive is damaged. Try again.",
        ));
        error.set_xalign(0.0);
        error.set_wrap(true);
        error.add_css_class("error");
        body.append(&error);
    }
    let password = gtk::PasswordEntry::new();
    password.set_show_peek_icon(true);
    password.set_activates_default(true);
    password.set_tooltip_text(Some("Archive password"));
    body.append(&dialogs::form_row("Password", &password));
    let hint = gtk::Label::new(Some(
        "Kept in memory for this session. Never saved to settings.",
    ));
    hint.add_css_class("dialog-hint");
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    body.append(&hint);
    root.append(&body);
    let actions = dialogs::dialog_actions();
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("dialog-button");
    let unlock = gtk::Button::with_label("Unlock");
    unlock.add_css_class("dialog-button");
    unlock.add_css_class("suggested-action");
    unlock.set_sensitive(false);
    password.connect_changed(glib::clone!(
        #[weak]
        unlock,
        move |password| unlock.set_sensitive(!password.text().is_empty())
    ));
    actions.append(&cancel);
    actions.append(&unlock);
    root.append(&actions);
    view.set_content(Some(&root));
    dialog.set_default_widget(Some(&unlock));
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    let reply = request.reply.clone();
    unlock.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        password,
        move |_| {
            if let Some(value) = Password::new(password.text().to_string()) {
                let _ = reply.try_send(Some(value));
                password.set_text("");
                dialog.close();
            }
        }
    ));
    dialog.connect_closed(glib::clone!(
        #[weak]
        password,
        move |_| {
            password.set_text("");
            let _ = request.reply.try_send(None);
        }
    ));
    let weak_dialog = dialog.downgrade();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let Some(dialog) = weak_dialog.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if request.cancel.is_cancelled() {
            dialog.close();
            return glib::ControlFlow::Break;
        }
        if !dialog.is_visible() {
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    dialog.present(Some(&window));
    password.grab_focus();
}
