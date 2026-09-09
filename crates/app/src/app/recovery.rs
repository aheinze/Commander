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

fn record_label(record: &RecoveryRecord) -> &'static str {
    if record.archive.is_some()
        || record
            .pending
            .iter()
            .chain(record.applied.iter())
            .any(|step| step.action == "update archive")
    {
        "Update archive"
    } else {
        job_kind_label(record.kind)
    }
}

fn report(record: &RecoveryRecord) -> String {
    let status = record.state.map_or_else(
        || "Interrupted — completion was not recorded".to_owned(),
        |state| state_label(state).to_owned(),
    );
    let mut lines = vec![
        format!("{} · {status}", record_label(record)),
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
    if record.archive.is_some() {
        lines.push("Restore archive replaces the current archive only if it still matches this saved update. A recovery copy of the current version is kept, and the restore can be undone.".into());
    }
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
                record_label(record),
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
                if record.archive.is_some() {
                    detail.add_response("archive", "Restore archive");
                    detail.set_response_appearance("archive", adw::ResponseAppearance::Destructive);
                }
                if !record.backups.is_empty() {
                    detail.add_response("restore", "Restore missing originals");
                }
                if !record.reviewed && record.state.is_none() {
                    detail.add_response("reviewed", "Mark reviewed");
                }
                let path = record.path.clone();
                let input = input.clone();
                detail.connect_response(None, move |_, response| match response {
                    "archive" => {
                        let _ = input.send(AppMsg::RestoreArchive(path.clone()));
                    }
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

    pub(super) fn restore_archive(
        &mut self,
        path: std::path::PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        if self.history_busy || self.active_operations > 0 {
            notifications::error("Wait for file operations to finish before restoring an archive");
            return;
        }
        let Some(change) = self
            .recovery_records
            .iter()
            .find(|record| record.path == path)
            .and_then(|record| record.archive.as_ref())
        else {
            return;
        };
        let expected = change.clone();
        let source = change.source.clone();
        let input = sender.input_sender().clone();
        self.history_busy = true;
        match thread::Builder::new()
            .name("commander-restore-archive".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let record = read_journal(&path).map_err(|e| e.to_string())?;
                    let change = record
                        .archive
                        .ok_or("The archive recovery record is no longer available")?;
                    if change != expected {
                        return Err(
                            "The recovery record changed. Reopen Recovery to review it.".into()
                        );
                    }
                    restore_archive_recorded(&change)
                }))
                .unwrap_or_else(|_| Err("Archive recovery stopped unexpectedly".into()));
                let _ = input.send(AppMsg::ArchiveRestoreReady { source, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.history_busy = false;
                notifications::error(&error.to_string());
            }
        }
    }

    pub(super) fn on_archive_restore_ready(
        &mut self,
        source: VPath,
        result: Result<dualpane_engine::journal::ArchiveChange, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.history_busy = false;
        match result {
            Ok(change) => {
                self.push_operation_log(format!(
                    "Restored {}. Recovery copy of previous version: {}",
                    change.source, change.backup
                ));
                self.record_history(HistoryEntry::Archive { change });
                notifications::success(
                    "Archive restored. Use Undo to return to the previous version.",
                );
            }
            Err(error) => notifications::error(&error),
        }
        self.on_archive_edited(source, sender);
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

/// Undo, redo, and manual recovery use the same verified, journaled transaction.
pub(super) fn restore_archive_recorded(
    change: &dualpane_engine::journal::ArchiveChange,
) -> Result<dualpane_engine::journal::ArchiveChange, String> {
    let journal = Arc::new(
        dualpane_engine::journal::JobJournal::create(
            &journal_directory(),
            JobKind::Copy,
            vec![change.source.clone()],
            Some(change.source.clone()),
        )
        .map_err(|e| e.message)?,
    );
    let cancel = CancelToken::new();
    let mut task = ArchiveTask::new(&cancel);
    task.set_journal(journal.clone());
    let result = crate::archive::edit::restore(change, &mut task);
    journal
        .append(&dualpane_engine::journal::JournalEvent::Finished {
            state: if result.is_ok() {
                JobState::Done
            } else {
                JobState::Failed
            },
            errors: result.as_ref().err().cloned().into_iter().collect(),
            completed_items: u64::from(result.is_ok()),
        })
        .map_err(|e| e.message)?;
    result
}
