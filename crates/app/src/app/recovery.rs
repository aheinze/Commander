use super::*;
use dualpane_engine::journal::{RecoveryRecord, read_journal, scan_journals};

pub(super) fn journal_directory() -> std::path::PathBuf {
    crate::session::state_directory()
        .unwrap_or_else(|| home_path().as_path().join(".local/state/dualpane"))
        .join("jobs")
}

fn state_label(state: JobState) -> &'static str {
    match state {
        JobState::Scanning => "Preparing",
        JobState::Running => "In progress",
        JobState::Paused => "Paused",
        JobState::Done => "Completed",
        JobState::Cancelled => "Cancelled",
        JobState::Failed => "Failed",
    }
}

fn report(record: &RecoveryRecord) -> String {
    let status = record.state.map_or_else(
        || "Interrupted — completion was not recorded".to_owned(),
        |state| state_label(state).to_owned(),
    );
    let mut lines = vec![
        format!("{} · {status}", job_kind_label(record.kind)),
        format!("{} committed item(s)", record.completed_items),
    ];
    if let Some(destination) = &record.destination {
        lines.push(format!("Destination: {destination}"));
    }
    for source in record.sources.iter().take(12) {
        lines.push(format!("Source: {source}"));
    }
    for transfer in record.transfers.iter().take(30) {
        lines.push(format!(
            "Completed: {} → {}{}",
            transfer.source,
            transfer.destination,
            if transfer.source_removed {
                " (source removed)"
            } else {
                ""
            }
        ));
    }
    for backup in record.backups.iter().take(30) {
        lines.push(format!(
            "Retained original: {}\n  Backup: {}",
            backup.original, backup.backup
        ));
    }
    for trash in record.trash.iter().take(20) {
        lines.push(format!("Trash: {} → {}", trash.original, trash.trashed));
    }
    if record.state != Some(JobState::Done) {
        for intent in record.applied.iter().rev().take(20) {
            lines.push(format!(
                "Completed step: {} · {}",
                intent.action, intent.target
            ));
        }
    }
    for intent in record.pending.iter().take(20) {
        lines.push(format!(
            "Check unfinished step: {} · {}{}",
            intent.action,
            intent
                .source
                .as_ref()
                .map_or_else(String::new, |source| format!("{source} → ")),
            intent.target
        ));
    }
    lines.extend(record.errors.iter().take(12).cloned());
    lines.push(format!("Full operation record: {}", record.path.display()));
    lines.push("Restore missing originals only fills absent paths. Existing files and temporary outputs remain available for inspection.".to_owned());
    lines.join("\n")
}

impl AppModel {
    pub(super) fn scan_recovery(&mut self, show: bool, sender: &ComponentSender<Self>) {
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("commander-recovery".to_owned())
            .spawn(move || {
                let (records, errors) = scan_journals(&journal_directory());
                let _ = input.send(AppMsg::RecoveryReady {
                    records,
                    errors,
                    show,
                });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not load recovery records: {error}"))
            }
        }
    }

    pub(super) fn show_recovery(&self, sender: &ComponentSender<Self>) {
        let heading = AlertSheet::new(
            Some("Recovery and saved operations"),
            Some(
                "Review completed work, interrupted steps, and retained originals. Overwrite backups stay available until you choose what to restore.",
            ),
        );
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        if self.recovery_records.is_empty() {
            content.append(&gtk::Label::new(Some("No saved file operations yet.")));
        }
        for record in self.recovery_records.iter().rev().take(100) {
            let state = record.state.map_or_else(
                || "Interrupted".to_owned(),
                |state| state_label(state).to_owned(),
            );
            let destination = record
                .destination
                .as_ref()
                .or_else(|| record.sources.first())
                .map_or_else(String::new, ToString::to_string);
            let button = gtk::Button::with_label(&format!(
                "{} · {state} · {} {}\n{destination}",
                job_kind_label(record.kind),
                record.completed_items,
                if record.completed_items == 1 {
                    "item"
                } else {
                    "items"
                }
            ));
            if let Some(label) = button.child().and_downcast::<gtk::Label>() {
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_max_width_chars(52);
            }
            button.set_tooltip_text(Some(&destination));
            let record = record.clone();
            let input = sender.input_sender().clone();
            button.connect_clicked(move |button| {
                let detail = AlertSheet::new(Some("Operation details"), Some(&report(&record)));
                detail.add_response("close", "Close");
                if !record.backups.is_empty() {
                    detail.add_response("restore", "Restore missing originals");
                }
                if !record.reviewed && record.state.is_none() {
                    detail.add_response("reviewed", "Mark reviewed");
                }
                let path = record.path.clone();
                let input = input.clone();
                detail.connect_response(None, move |_, response| match response {
                    "restore" => {
                        let _ = input.send(AppMsg::RestoreMissingOriginals(path.clone()));
                    }
                    "reviewed" => {
                        let _ = input.send(AppMsg::ReviewRecovery(path.clone()));
                    }
                    _ => {}
                });
                detail.present(Some(button));
            });
            content.append(&button);
        }
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .max_content_height(420)
            .propagate_natural_height(true)
            .child(&content)
            .build();
        heading.set_extra_child(Some(&scroll));
        heading.add_response("close", "Close");
        heading.set_close_response("close");
        heading.present(relm4::main_application().active_window().as_ref());
        if !self.recovery_errors.is_empty() {
            notifications::error(&self.recovery_errors.join("\n"));
        }
    }

    pub(super) fn restore_missing_originals(
        &mut self,
        path: std::path::PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        if !self
            .recovery_records
            .iter()
            .any(|record| record.path == path)
        {
            return;
        }
        let input = sender.input_sender().clone();
        let vfs = Arc::clone(&self.vfs);
        match thread::Builder::new()
            .name("commander-restore-originals".to_owned())
            .spawn(move || {
                let result = read_journal(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|record| restore_absent_backups(vfs.as_ref(), &record));
                let _ = input.send(AppMsg::RecoveryFinished(result));
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => self.pane_mut(self.active_pane).error = Some(error.to_string()),
        }
    }

    pub(super) fn review_recovery(
        &mut self,
        path: std::path::PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        if !self
            .recovery_records
            .iter()
            .any(|record| record.path == path)
        {
            return;
        }
        let input = sender.input_sender().clone();
        match thread::Builder::new().name("commander-review-recovery".to_owned()).spawn(move || {
            let result = dualpane_engine::journal::mark_reviewed(&path).map(|()| "Marked reviewed. The operation record and retained files are still available.".to_owned()).map_err(|error| error.to_string());
            let _ = input.send(AppMsg::RecoveryFinished(result));
        }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => self.pane_mut(self.active_pane).error = Some(error.to_string()),
        }
    }
}

pub(super) fn restore_absent_backups(
    vfs: &dyn Vfs,
    record: &RecoveryRecord,
) -> Result<String, String> {
    let mut restored = 0;
    let mut kept = 0;
    let mut errors = Vec::new();
    for backup in &record.backups {
        match vfs.stat(&backup.original, false) {
            Ok(_) => {
                kept += 1;
                continue;
            }
            Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => {}
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        }
        let actual = match vfs.stat(&backup.backup, false) {
            Ok(actual) => actual,
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        };
        if actual.identity != backup.metadata.identity
            || actual.kind != backup.metadata.kind
            || (actual.kind != EntryKind::Directory
                && (actual.size != backup.metadata.size
                    || actual.modified != backup.metadata.modified))
        {
            errors.push(format!(
                "{} changed; inspect it before restoring",
                backup.backup
            ));
            continue;
        }
        match vfs.rename_noreplace(&backup.backup, &backup.original) {
            Ok(()) => {
                restored += 1;
                if let Some(parent) = backup.original.parent()
                    && let Err(error) = vfs.sync_file(&parent)
                {
                    errors.push(error.to_string());
                }
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    let mut message =
        format!("Restored {restored} original(s). Kept {kept} existing destination(s).");
    if !errors.is_empty() {
        message.push_str(&format!("\n{}", errors.join("\n")));
        return Err(message);
    }
    Ok(message)
}
