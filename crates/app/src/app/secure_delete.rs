//! Reviewed best-effort overwriting of local files, with independent job controls.
use super::*;
use dualpane_platform::secure_delete::{
    SecureDeletePlan, SecureDeleteProgress, review_secure_delete, secure_delete,
};

pub(super) struct Review {
    dialog: adw::Dialog,
    status: notifications::Feedback,
    acknowledge: gtk::CheckButton,
    erase: gtk::Button,
    plan: RefCell<Option<SecureDeletePlan>>,
    cancel: CancelToken,
    closed: Cell<bool>,
}

fn label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_max_width_chars(68);
    label
}

fn show(pane: PaneId, paths: Vec<VPath>, input: relm4::Sender<AppMsg>) -> Option<Rc<Review>> {
    let parent = relm4::main_application()
        .active_window()
        .or_else(|| relm4::main_application().windows().first().cloned())?;
    let dialog = adw::Dialog::builder()
        .title("Secure delete")
        .content_width(600)
        .follows_content_size(true)
        .presentation_mode(adw::DialogPresentationMode::Floating)
        .build();
    dialog.add_css_class("utility-dialog");
    dialog.add_css_class("secure-delete-dialog");
    let view = adw::ToolbarView::new();
    view.add_top_bar(&chrome::dialog_header(&dialog));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_start(18);
    root.set_margin_end(18);
    root.set_margin_top(8);
    root.set_margin_bottom(18);
    root.append(&label("Overwrite file contents, sync to disk, verify the overwrite, then delete. Files bypass Trash and cannot be restored with Undo."));
    let limitation = label(
        "Recovery may still be possible on SSDs, flash drives, and filesystems with snapshots or copy-on-write. Backups and other copies are not erased.",
    );
    root.append(&limitation);
    let status = notifications::Feedback::default();
    let names: Vec<_> = paths.iter().map(ToString::to_string).collect();
    let strings = gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let name = gtk::Label::new(None);
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_width_chars(1);
        name.set_margin_start(10);
        name.set_margin_end(10);
        name.set_margin_top(8);
        name.set_margin_bottom(8);
        item.set_child(Some(&name));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let value = item.item().and_downcast::<gtk::StringObject>().unwrap();
        let name = item.child().and_downcast::<gtk::Label>().unwrap();
        name.set_label(&value.string());
        name.set_tooltip_text(Some(&value.string()));
    });
    let list = gtk::ListView::new(Some(gtk::NoSelection::new(Some(strings))), Some(factory));
    list.add_css_class("boxed-list");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(40)
        .max_content_height(180)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    root.append(&scroll);
    let note = label(
        "Local regular files only. Folders, symbolic links, and files with hard links are not supported. Stopping cannot undo bytes already overwritten.",
    );
    root.append(&note);
    let acknowledge =
        gtk::CheckButton::with_label("I understand that this cannot be undone in Commander");
    if let Some(text) = acknowledge.child().and_downcast::<gtk::Label>() {
        text.set_wrap(true);
        text.set_xalign(0.0);
    }
    root.append(&acknowledge);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.add_css_class("dialog-actions");
    let cancel = gtk::Button::with_label("Cancel");
    let erase = gtk::Button::with_label("Overwrite and delete");
    erase.add_css_class("destructive-action");
    erase.set_sensitive(false);
    actions.append(&cancel);
    actions.append(&erase);
    root.append(&actions);
    view.set_content(Some(&root));
    dialog.set_child(Some(&view));
    dialog.set_default_widget(Some(&cancel));
    let state = Rc::new(Review {
        dialog,
        status,
        acknowledge,
        erase,
        plan: RefCell::new(None),
        cancel: CancelToken::new(),
        closed: Cell::new(false),
    });
    {
        let weak = Rc::downgrade(&state);
        state.acknowledge.connect_toggled(move |button| {
            if let Some(state) = weak.upgrade() {
                state
                    .erase
                    .set_sensitive(button.is_active() && state.plan.borrow().is_some());
            }
        });
    }
    {
        let weak = Rc::downgrade(&state);
        state.erase.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade()
                && state.acknowledge.is_active()
            {
                let plan = state.plan.borrow_mut().take();
                if let Some(plan) = plan {
                    let _ = input.send(AppMsg::SecureDeleteConfirmed { pane, plan });
                    state.dialog.close();
                }
            }
        });
    }
    {
        let dialog = state.dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
        let keep_alive = RefCell::new(Some(Rc::clone(&state)));
        state.dialog.connect_closed(move |_| {
            if let Some(state) = keep_alive.borrow_mut().take() {
                state.closed.set(true);
                state.cancel.cancel();
                state.plan.borrow_mut().take();
            }
        });
    }
    state.dialog.present(Some(&parent));
    cancel.grab_focus();
    let paths: Vec<_> = paths
        .iter()
        .map(|path| path.as_path().to_path_buf())
        .collect();
    let token = state.cancel.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = thread::Builder::new()
        .name("commander-erase-review".into())
        .spawn(move || {
            let result = review_secure_delete(&paths, &mut || {
                token
                    .check()
                    .map_err(|_| std::io::Error::other("Review cancelled"))
            });
            let _ = tx.send(result);
        });
    if let Err(error) = worker {
        state
            .status
            .error(&format!("Could not review files: {error}"));
        return Some(state);
    }
    let weak = Rc::downgrade(&state);
    glib::timeout_add_local(Duration::from_millis(40), move || {
        let Some(state) = weak.upgrade().filter(|state| !state.closed.get()) else {
            return glib::ControlFlow::Break;
        };
        match rx.try_recv() {
            Ok(Ok(plan)) => {
                let noun = if plan.len() == 1 { "file" } else { "files" };
                state.status.info(&format!(
                    "{} {noun} · {} · ready to overwrite",
                    plan.len(),
                    format_size(plan.bytes, EntryKind::File)
                ));
                state.plan.replace(Some(plan));
                state.erase.set_sensitive(state.acknowledge.is_active());
            }
            Ok(Err(error)) => {
                state
                    .status
                    .error(&format!("Cannot securely delete this selection: {error}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(_) => state
                .status
                .error("Review stopped unexpectedly. Close this dialog and try again."),
        }
        glib::ControlFlow::Break
    });
    Some(state)
}

impl AppModel {
    pub(super) fn review_secure_delete(&mut self, sender: &ComponentSender<Self>) {
        let paths = self.operation_sources(self.active_pane);
        if self.history_busy || self.active_operations > 0 {
            self.pane_mut(self.active_pane).error = Some(
                "Wait for current file operations to finish before securely deleting files".into(),
            );
            return;
        }
        if paths.is_empty()
            || paths
                .iter()
                .any(|path| self.is_archive_browse_path(path) || !path.as_path().is_absolute())
        {
            self.pane_mut(self.active_pane).error =
                Some("Select local files outside archives to securely delete".into());
            return;
        }
        show(self.active_pane, paths, sender.input_sender().clone());
    }

    pub(super) fn start_secure_delete(
        &mut self,
        pane: PaneId,
        plan: SecureDeletePlan,
        sender: &ComponentSender<Self>,
    ) {
        if self.history_busy || self.active_operations > 0 || plan.is_empty() {
            self.pane_mut(pane).error = Some("File operations are active. Wait for them to finish, then review secure deletion again.".into());
            return;
        }
        let sources: Vec<_> = plan.paths().map(VPath::from).collect();
        if sources.iter().any(|path| self.is_archive_browse_path(path)) {
            self.pane_mut(pane).error = Some(
                "Secure delete is not available inside archives. Use Remove from archive instead."
                    .into(),
            );
            return;
        }
        let id = JobId::next();
        let control = JobControl::new();
        self.operations.insert(
            id,
            OperationStatus {
                phase: JobPhase::Preparing,
                kind: OperationKind::SecureDelete,
                state: JobState::Running,
                progress: JobProgress {
                    bytes_total: plan.bytes.saturating_mul(2),
                    items_total: plan.len() as u64,
                    ..JobProgress::default()
                },
                control: control.clone(),
                commands: std::sync::mpsc::channel().0,
                retry: OperationRetry::SecureDelete { sources },
                waiting_for_conflict: false,
                error: None,
            },
        );
        self.active_operations = self.active_operations.saturating_add(1);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name(format!("commander-erase-{}", id.get()))
            .spawn(move || {
                let mut last = Instant::now();
                let mut previous_phase = None;
                let mut previous_count = 0;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    secure_delete(
                        &plan,
                        &mut || {
                            control
                                .checkpoint()
                                .map_err(|_| std::io::Error::other("Secure deletion stopped"))
                        },
                        &mut |progress| {
                            if last.elapsed() >= Duration::from_millis(50)
                                || previous_phase != Some(progress.verifying)
                                || previous_count != progress.files_done
                            {
                                last = Instant::now();
                                previous_phase = Some(progress.verifying);
                                previous_count = progress.files_done;
                                let _ = input.send(AppMsg::SecureDeleteProgress { id, progress });
                            }
                        },
                    )
                    .map_err(|error| error.to_string())
                }))
                .unwrap_or_else(|_| Err("Secure deletion stopped unexpectedly".into()));
                let _ = input.send(AppMsg::SecureDeleteReady { id, pane, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => self.on_secure_delete_ready(
                id,
                pane,
                Err(format!("Could not start secure deletion: {error}")),
                sender,
            ),
        }
    }

    pub(super) fn on_secure_delete_progress(&mut self, id: JobId, progress: SecureDeleteProgress) {
        if let Some(operation) = self
            .operations
            .get_mut(&id)
            .filter(|operation| operation.is_active())
        {
            operation.phase = if progress.verifying {
                JobPhase::Verifying
            } else {
                JobPhase::Copying
            };
            operation.progress.bytes_done = progress.bytes_done;
            operation.progress.items_done = progress.files_done as u64;
            operation.progress.current_path = Some(VPath::from(progress.path));
        }
    }

    pub(super) fn on_secure_delete_ready(
        &mut self,
        id: JobId,
        pane: PaneId,
        result: Result<usize, String>,
        sender: &ComponentSender<Self>,
    ) {
        let Some(operation) = self
            .operations
            .get_mut(&id)
            .filter(|operation| operation.is_active())
        else {
            return;
        };
        let message = match result {
            Ok(count) => {
                operation.state = JobState::Done;
                operation.progress.items_done = count as u64;
                format!(
                    "Secure delete: {count} file(s) overwritten, verified, and deleted. Storage snapshots and other copies are not erased."
                )
            }
            Err(error) => {
                operation.state = if operation.control.cancel_token().is_cancelled() {
                    JobState::Cancelled
                } else {
                    JobState::Failed
                };
                let message = format!(
                    "{error}. {} file(s) already deleted. Remaining files may be partially overwritten; this cannot be undone. Review the files again before another attempt.",
                    operation.progress.items_done
                );
                operation.error = Some(message.clone());
                self.pane_mut(pane).error = Some(message.clone());
                message
            }
        };
        self.push_operation_log(message);
        self.active_operations = self.active_operations.saturating_sub(1);
        self.prune_finished_operations();
        self.start_listing(PaneId::Left, sender);
        self.start_listing(PaneId::Right, sender);
    }
}

#[cfg(test)]
mod tests;
