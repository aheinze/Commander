use super::*;

impl OperationStatus {
    pub(super) fn is_active(&self) -> bool {
        matches!(
            self.state,
            JobState::Scanning | JobState::Running | JobState::Paused
        )
    }

    pub(super) fn can_control(&self) -> bool {
        self.is_active() && !self.control.cancel_token().is_cancelled()
    }
}

pub(super) fn first_active_operation(
    operations: &BTreeMap<JobId, OperationStatus>,
) -> Option<JobId> {
    operations
        .iter()
        .find_map(|(id, operation)| operation.can_control().then_some(*id))
}

fn featured_operation(operations: &BTreeMap<JobId, OperationStatus>) -> Option<JobId> {
    operations
        .iter()
        .find_map(|(id, operation)| operation.is_active().then_some(*id))
        .or_else(|| operations.keys().next_back().copied())
}

#[derive(Debug, PartialEq)]
struct JobPresentation {
    title: String,
    status: &'static str,
    detail: String,
    path: String,
    error: Option<String>,
    fraction: Option<f64>,
    busy: bool,
    paused: bool,
    can_pause: bool,
    can_cancel: bool,
    can_retry: bool,
    low_space: bool,
    checking_space: bool,
    finished: bool,
}

impl JobPresentation {
    fn new(operation: &OperationStatus) -> Self {
        let active = operation.is_active();
        let cancelling = active && operation.control.cancel_token().is_cancelled();
        let paused = active && operation.control.is_paused();
        let space = operation.control.space_issue().filter(|_| active);
        let values = &operation.progress;
        // Trash and delete report items; their scanned byte totals are not bytes copied.
        let use_bytes = matches!(
            operation.kind,
            OperationKind::Files(JobKind::Copy | JobKind::Move) | OperationKind::SecureDelete
        ) && values.bytes_total > 0;
        let (done, total) = if use_bytes {
            (values.bytes_done, values.bytes_total)
        } else {
            (values.items_done, values.items_total)
        };
        let fraction = if operation.state == JobState::Done {
            Some(1.0)
        } else if operation.state == JobState::Scanning || total == 0 {
            None
        } else {
            Some((done as f64 / total as f64).clamp(0.0, 1.0))
        };
        let status = if cancelling {
            "Cancelling…"
        } else if active && operation.waiting_for_conflict {
            "Needs a decision"
        } else if let Some(space) = &space {
            if space.checking {
                "Checking free space…"
            } else if space.available_bytes.is_none() {
                "Free space unavailable"
            } else {
                "Not enough space"
            }
        } else if paused {
            "Paused"
        } else {
            match operation.state {
                JobState::Scanning => "Preparing…",
                JobState::Running | JobState::Paused => match operation.phase {
                    JobPhase::Verifying => "Verifying…",
                    JobPhase::Finishing => "Finishing…",
                    _ if fraction == Some(1.0) => "Finishing…",
                    _ => "In progress",
                },
                JobState::Done => "Completed",
                JobState::Cancelled => "Cancelled",
                JobState::Failed => "Failed",
            }
        };
        let verb = match operation.kind {
            OperationKind::Files(JobKind::Copy) => "Copying",
            OperationKind::Files(JobKind::Move) => "Moving",
            OperationKind::Files(JobKind::Trash) => "Moving to Trash",
            OperationKind::Files(JobKind::DeletePermanent) => "Deleting",
            OperationKind::CreateArchive => "Creating archive",
            OperationKind::ExtractArchive => "Extracting archive",
            OperationKind::UpdateArchive => "Updating archive",
            OperationKind::SecureDelete => "Overwriting files",
        };
        let title = if status == "In progress" {
            verb.to_owned()
        } else {
            format!("{} · {status}", operation.kind.label())
        };
        let mut detail = if operation.state == JobState::Scanning {
            "Counting files and folders…".to_owned()
        } else if operation.kind.is_archive() {
            format!(
                "{} items · {} processed",
                values.items_done,
                format_size(values.bytes_done, EntryKind::File)
            )
        } else if use_bytes {
            format!(
                "{} / {}",
                format_size(values.bytes_done, EntryKind::File),
                format_size(values.bytes_total, EntryKind::File)
            )
        } else if total > 0 {
            format!("{done} / {total} items")
        } else {
            status.to_owned()
        };
        if status == "In progress"
            && use_bytes
            && values.throughput_bytes_per_second.is_finite()
            && values.throughput_bytes_per_second > 0.0
        {
            detail.push_str(&format!(
                " · {}/s",
                format_size(values.throughput_bytes_per_second as u64, EntryKind::File)
            ));
            if let Some(eta) = values.eta {
                detail.push_str(&format!(" · {} remaining", format_eta(eta)));
            }
        }
        if let Some(space) = &space {
            detail = format!(
                "{} needed · {} available",
                format_size(space.required_bytes, EntryKind::File),
                space.available_bytes.map_or_else(
                    || "Unknown".into(),
                    |bytes| format_size(bytes, EntryKind::File)
                )
            );
            if let Some(error) = &space.error {
                detail.push_str(&format!(" — {error}"));
            }
        }
        let mut path = values
            .current_path
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                let sources = match &operation.retry {
                    OperationRetry::UpdateArchive { request, .. } => {
                        return request.mount.source.to_string();
                    }
                    OperationRetry::Copy { sources, .. }
                    | OperationRetry::Move { sources, .. }
                    | OperationRetry::Trash { sources, .. }
                    | OperationRetry::Delete { sources, .. }
                    | OperationRetry::SecureDelete { sources }
                    | OperationRetry::Archive { sources, .. } => sources,
                };
                sources
                    .first()
                    .map_or_else(String::new, ToString::to_string)
            });
        if let OperationRetry::Copy { destination, .. }
        | OperationRetry::Move { destination, .. }
        | OperationRetry::Archive { destination, .. } = &operation.retry
        {
            path.push_str(&format!(" → {destination}"));
        }
        Self {
            title,
            status,
            detail,
            path,
            fraction,
            error: operation.error.clone(),
            low_space: space.is_some(),
            checking_space: space.as_ref().is_some_and(|space| space.checking),
            busy: space.as_ref().is_some_and(|space| space.checking)
                || (active && !paused && !operation.waiting_for_conflict),
            paused,
            can_pause: operation.can_control() && !operation.waiting_for_conflict,
            can_cancel: operation.can_control(),
            can_retry: matches!(operation.state, JobState::Cancelled | JobState::Failed)
                && !operation.recovery.retried
                && (!matches!(operation.kind, OperationKind::Files(_)) || operation.recovery.ready)
                && !matches!(
                    operation.kind,
                    OperationKind::ExtractArchive
                        | OperationKind::SecureDelete
                        | OperationKind::UpdateArchive
                ),
            finished: !active,
        }
    }
}

fn format_eta(eta: Duration) -> String {
    let seconds = eta.as_secs();
    if seconds < 60 {
        format!("{}s", seconds.max(1))
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    }
}

fn ellipsized_label(class: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    // Keep long paths and errors from increasing the window's minimum width.
    label.set_width_chars(1);
    label.add_css_class(class);
    label
}

fn connect_job_button(
    button: &gtk::Button,
    sender: &relm4::Sender<AppMsg>,
    id: JobId,
    message: fn(JobId) -> AppMsg,
) {
    let input = sender.clone();
    button.connect_clicked(move |_| {
        let _ = input.send(message(id));
    });
}

fn render_progress(progress: &gtk::ProgressBar, view: &JobPresentation) {
    progress.set_fraction(view.fraction.unwrap_or(0.0));
    progress.set_visible(view.fraction.is_some());
}

struct JobRowWidgets {
    root: gtk::Box,
    header: gtk::Box,
    actions: gtk::Box,
    title: gtk::Label,
    status: gtk::Label,
    detail: gtk::Label,
    path: gtk::Label,
    progress: gtk::ProgressBar,
    spinner: gtk::Spinner,
    recheck: gtk::Button,
    pause: gtk::Button,
    cancel: gtk::Button,
    retry: gtk::Button,
    dismiss: gtk::Button,
    rendered: Option<JobPresentation>,
}

impl JobRowWidgets {
    fn new(id: JobId, sender: &relm4::Sender<AppMsg>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 3);
        root.add_css_class("operation-card");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = ellipsized_label("operation-title");
        let status = gtk::Label::new(None);
        status.add_css_class("operation-state");
        let spinner = gtk::Spinner::new();
        spinner.set_size_request(12, 12);
        spinner.set_valign(gtk::Align::Center);
        header.append(&spinner);
        header.append(&title);
        root.append(&header);
        let progress = gtk::ProgressBar::new();
        progress.add_css_class("operation-progress");
        root.append(&progress);
        let detail = ellipsized_label("operation-path");
        detail.set_wrap(true);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::None);
        let path = ellipsized_label("operation-path");
        root.append(&detail);
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.append(&path);
        footer.append(&status);
        root.append(&footer);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        actions.set_halign(gtk::Align::End);
        let recheck = gtk::Button::with_label("Recheck space");
        let pause = icon_button("commander-pause-symbolic", "Pause operation");
        let cancel = icon_button("commander-circle-x-symbolic", "Cancel operation");
        let retry = icon_button("commander-rotate-cw-symbolic", "Retry operation");
        let dismiss = icon_button("commander-x-symbolic", "Dismiss finished operation");
        for button in [&recheck, &pause, &cancel, &retry, &dismiss] {
            button.add_css_class("flat");
            button.add_css_class("operation-action");
            button.set_valign(gtk::Align::Center);
            actions.append(button);
        }
        cancel.add_css_class("operation-action-danger");
        connect_job_button(&recheck, sender, id, AppMsg::RecheckOperationSpace);
        connect_job_button(&pause, sender, id, AppMsg::TogglePauseOperation);
        connect_job_button(&cancel, sender, id, AppMsg::CancelOperation);
        connect_job_button(&retry, sender, id, AppMsg::RetryOperation);
        connect_job_button(&dismiss, sender, id, AppMsg::DismissOperation);
        header.append(&actions);
        Self {
            root,
            header,
            actions,
            title,
            status,
            detail,
            path,
            progress,
            spinner,
            recheck,
            pause,
            cancel,
            retry,
            dismiss,
            rendered: None,
        }
    }

    fn render(&mut self, operation: &OperationStatus) {
        let view = JobPresentation::new(operation);
        if self.rendered.as_ref() == Some(&view) {
            return;
        }
        // Text actions need their own row in the narrow Jobs popover.
        if self
            .rendered
            .as_ref()
            .is_some_and(|old| old.low_space != view.low_space)
            || (self.rendered.is_none() && view.low_space)
        {
            if let Some(parent) = self.actions.parent().and_downcast::<gtk::Box>() {
                parent.remove(&self.actions);
            }
            if view.low_space {
                self.root.append(&self.actions);
            } else {
                self.header.append(&self.actions);
            }
        }
        self.actions.set_spacing(if view.low_space { 8 } else { 2 });
        self.title.set_label(operation.kind.label());
        self.status.set_label(&match view.fraction {
            Some(fraction) if view.status == "In progress" => {
                format!("{:.0}%", (fraction * 100.0).floor())
            }
            _ => view.status.to_owned(),
        });
        self.detail.set_label(&view.detail);
        self.detail.set_tooltip_text(Some(&view.detail));
        self.path.set_label(&view.path);
        self.path.set_tooltip_text(Some(&view.path));
        self.spinner.set_spinning(view.busy);
        self.spinner.set_visible(view.busy);
        if view.low_space {
            if self.pause.label().as_deref() != Some("Resume anyway") {
                self.pause.set_label("Resume anyway");
            }
        } else {
            self.pause.set_icon_name(if view.paused {
                "commander-play-symbolic"
            } else {
                "commander-pause-symbolic"
            });
        }
        let label = if view.low_space {
            "Resume anyway"
        } else if view.paused {
            "Resume operation"
        } else {
            "Pause operation"
        };
        self.pause.set_tooltip_text(Some(label));
        self.pause
            .update_property(&[gtk::accessible::Property::Label(label)]);
        self.recheck.set_visible(view.low_space);
        self.recheck
            .set_sensitive(view.can_cancel && !view.checking_space);
        self.pause.set_visible(!view.finished);
        self.pause.set_sensitive(view.can_pause);
        self.cancel.set_visible(!view.finished);
        self.cancel.set_sensitive(view.can_cancel);
        self.retry.set_visible(view.can_retry);
        self.retry.set_tooltip_text(Some("Retry unfinished items"));
        self.dismiss.set_visible(view.finished);
        render_progress(&self.progress, &view);
        self.rendered = Some(view);
    }
}

/// Keyed rows keep focus and button presses intact while progress arrives at 20 Hz.
pub(super) struct JobListWidgets {
    pub(super) root: gtk::Box,
    rows: RefCell<BTreeMap<JobId, JobRowWidgets>>,
}

impl JobListWidgets {
    pub(super) fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("operation-list");
        Self {
            root,
            rows: RefCell::new(BTreeMap::new()),
        }
    }

    pub(super) fn render(
        &self,
        operations: &BTreeMap<JobId, OperationStatus>,
        sender: &relm4::Sender<AppMsg>,
    ) {
        let mut rows = self.rows.borrow_mut();
        rows.retain(|id, row| {
            if operations.contains_key(id) {
                true
            } else {
                self.root.remove(&row.root);
                false
            }
        });
        for (id, operation) in operations {
            let row = rows.entry(*id).or_insert_with(|| {
                let row = JobRowWidgets::new(*id, sender);
                self.root.prepend(&row.root);
                row
            });
            row.render(operation);
        }
    }
}

pub(super) struct JobActivityWidgets {
    pub(super) root: gtk::Box,
    title: gtk::Label,
    spinner: gtk::Spinner,
    state_icon: gtk::Image,
    progress: gtk::ProgressBar,
    recheck: gtk::Button,
    pause: gtk::Button,
    cancel: gtk::Button,
    dismiss: gtk::Button,
    menu: gtk::MenuButton,
    list: JobListWidgets,
    featured: Rc<Cell<Option<JobId>>>,
}

impl JobActivityWidgets {
    pub(super) fn new(sender: &relm4::Sender<AppMsg>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        root.add_css_class("job-activity");
        root.set_widget_name("job-activity");
        root.set_visible(false);
        let spinner = gtk::Spinner::new();
        spinner.set_valign(gtk::Align::Center);
        spinner.set_size_request(12, 12);
        root.append(&spinner);
        let state_icon = gtk::Image::new();
        state_icon.set_pixel_size(12);
        state_icon.add_css_class("job-activity-state");
        root.append(&state_icon);
        let title = ellipsized_label("job-activity-title");
        title.set_single_line_mode(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&title);
        let progress = gtk::ProgressBar::new();
        progress.add_css_class("job-activity-progress");
        progress.set_hexpand(false);
        progress.set_valign(gtk::Align::Center);
        root.append(&progress);
        let featured = Rc::new(Cell::new(None));
        let recheck = gtk::Button::with_label("Recheck space");
        let pause = icon_button("commander-pause-symbolic", "Pause operation");
        let cancel = icon_button("commander-circle-x-symbolic", "Cancel operation");
        let dismiss = icon_button("commander-x-symbolic", "Dismiss finished operation");
        for (button, message) in [
            (
                &recheck,
                AppMsg::RecheckOperationSpace as fn(JobId) -> AppMsg,
            ),
            (&pause, AppMsg::TogglePauseOperation),
            (&cancel, AppMsg::CancelOperation),
            (&dismiss, AppMsg::DismissOperation),
        ] {
            button.add_css_class("flat");
            button.add_css_class("job-activity-action");
            button.set_valign(gtk::Align::Center);
            let selected = Rc::clone(&featured);
            let input = sender.clone();
            button.connect_clicked(move |_| {
                if let Some(id) = selected.get() {
                    let _ = input.send(message(id));
                }
            });
            root.append(button);
        }
        let list = JobListWidgets::new();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_width(280)
            .max_content_height(360)
            .propagate_natural_height(true)
            .child(&list.root)
            .build();
        let popover = gtk::Popover::new();
        popover.add_css_class("job-popover");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Top);
        popover.set_child(Some(&scroll));
        let menu = gtk::MenuButton::new();
        menu.add_css_class("job-activity-menu");
        menu.set_valign(gtk::Align::Center);
        menu.set_label("Jobs");
        menu.set_tooltip_text(Some("Show active and recent file operations"));
        menu.update_property(&[gtk::accessible::Property::Label("File operations")]);
        menu.set_popover(Some(&popover));
        root.append(&menu);
        Self {
            root,
            title,
            spinner,
            state_icon,
            progress,
            recheck,
            pause,
            cancel,
            dismiss,
            menu,
            list,
            featured,
        }
    }

    pub(super) fn render(
        &self,
        operations: &BTreeMap<JobId, OperationStatus>,
        sender: &relm4::Sender<AppMsg>,
    ) {
        self.list.render(operations, sender);
        let id = featured_operation(operations);
        self.featured.set(id);
        self.root.set_visible(id.is_some());
        let Some(operation) = id.and_then(|id| operations.get(&id)) else {
            self.menu.popdown();
            self.spinner.stop();
            return;
        };
        let view = JobPresentation::new(operation);
        let title = match view.fraction {
            Some(fraction) if view.status == "In progress" => {
                format!("{} · {:.0}%", view.title, (fraction * 100.0).floor())
            }
            _ => view.title.clone(),
        };
        self.title.set_label(&if view.low_space {
            format!("{title} · {}", view.detail)
        } else {
            title.clone()
        });
        let detail = view.error.as_deref().unwrap_or(&view.detail);
        self.title
            .set_tooltip_text(Some(&format!("{title}\n{detail}\n{}", view.path)));
        self.title
            .update_property(&[gtk::accessible::Property::Description(detail)]);
        let indeterminate = view.busy && view.fraction.is_none();
        self.spinner.set_spinning(indeterminate);
        self.spinner.set_visible(indeterminate);
        self.state_icon.set_visible(!view.busy);
        self.state_icon.set_icon_name(Some(
            if operation.state == JobState::Failed
                || operation.waiting_for_conflict
                || view.low_space
            {
                "commander-triangle-alert-symbolic"
            } else if view.paused {
                "commander-pause-symbolic"
            } else if operation.state == JobState::Done {
                "commander-circle-check-symbolic"
            } else {
                "commander-circle-x-symbolic"
            },
        ));
        if operation.state == JobState::Failed || operation.waiting_for_conflict || view.low_space {
            self.root.add_css_class("job-activity-attention");
        } else {
            self.root.remove_css_class("job-activity-attention");
        }
        self.recheck.set_visible(view.low_space);
        self.recheck
            .set_sensitive(view.can_cancel && !view.checking_space);
        self.pause.set_visible(!view.finished);
        self.pause.set_sensitive(view.can_pause);
        if view.low_space {
            if self.pause.label().as_deref() != Some("Resume anyway") {
                self.pause.set_label("Resume anyway");
            }
        } else {
            self.pause.set_icon_name(if view.paused {
                "commander-play-symbolic"
            } else {
                "commander-pause-symbolic"
            });
        }
        let label = if view.low_space {
            "Resume anyway"
        } else if view.paused {
            "Resume operation"
        } else {
            "Pause operation"
        };
        self.pause.set_tooltip_text(Some(label));
        self.pause
            .update_property(&[gtk::accessible::Property::Label(label)]);
        self.cancel.set_visible(!view.finished);
        self.cancel.set_sensitive(view.can_cancel);
        self.dismiss.set_visible(view.finished);
        let active = operations
            .values()
            .filter(|operation| operation.is_active())
            .count();
        self.menu.set_label(&if active > 0 {
            format!("Jobs ({active})")
        } else {
            format!("Recent ({})", operations.len())
        });
        render_progress(&self.progress, &view);
        self.progress
            .set_visible(view.fraction.is_some() && !view.finished);
    }
}

#[cfg(test)]
#[path = "job_view_tests.rs"]
mod tests;
