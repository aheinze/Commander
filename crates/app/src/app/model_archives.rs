//! Archive workers share the visible job list, while retaining independent controls.
use super::*;

#[cfg(test)]
#[path = "archive_view_tests.rs"]
mod tests;

impl AppModel {
    pub(super) fn start_archive_operation(
        &mut self,
        retry: OperationRetry,
        sender: &ComponentSender<Self>,
    ) {
        let OperationRetry::Archive {
            pane,
            sources,
            destination,
            format,
            password,
        } = retry.clone()
        else {
            return;
        };
        if self.history_busy {
            self.pane_mut(pane).error =
                Some("Wait for undo or redo to finish before changing files".to_owned());
            return;
        }
        if sources.is_empty() {
            return;
        }
        if self.is_archive_browse_path(&destination) {
            self.pane_mut(pane).error =
                Some("Create or extract this archive in a regular folder, then copy the result into the archive.".to_owned());
            return;
        }
        let kind = if format.is_some() {
            OperationKind::CreateArchive
        } else {
            OperationKind::ExtractArchive
        };
        let id = JobId::next();
        let control = JobControl::new();
        self.operations.insert(
            id,
            OperationStatus {
                phase: JobPhase::Preparing,
                kind,
                state: JobState::Running,
                progress: JobProgress::default(),
                control: control.clone(),
                commands: std::sync::mpsc::channel().0,
                retry,
                waiting_for_conflict: false,
                error: None,
            },
        );
        self.active_operations = self.active_operations.saturating_add(1);
        let input = sender.input_sender().clone();
        let vfs = Arc::clone(&self.vfs);
        match thread::Builder::new()
            .name(format!("commander-archive-{}", id.get()))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let cancel = control.cancel_token();
                    let mut task = ArchiveTask::with_progress(&control, |progress| {
                        let _ = input.send(AppMsg::ArchiveProgress { id, progress });
                    });
                    task.set_password(password);
                    task.set_password_prompt(|source, incorrect| {
                        archive_password::request(
                            &input,
                            archive_password::Context::Operation(id),
                            source,
                            incorrect,
                            &cancel,
                        )
                    });
                    let result = if let Some(format) = format {
                        create_archive(vfs.as_ref(), &sources, &destination, format, &mut task)
                    } else {
                        extract_archive(vfs.as_ref(), &sources[0], &destination, &mut task)
                    };
                    task.flush();
                    result
                }))
                .unwrap_or_else(|_| Err("The archive worker stopped unexpectedly".to_owned()));
                let _ = input.send(AppMsg::ArchiveReady { id, pane, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => self.on_archive_ready(
                id,
                pane,
                Err(format!("Could not start archive operation: {error}")),
                sender,
            ),
        }
    }

    pub(super) fn on_archive_progress(&mut self, id: JobId, progress: ArchiveProgress) {
        if let Some(operation) = self.operations.get_mut(&id)
            && operation.is_active()
        {
            operation.phase = if progress.finishing {
                JobPhase::Finishing
            } else {
                JobPhase::Copying
            };
            operation.progress.bytes_done = progress.bytes_done;
            operation.progress.items_done = progress.items_done;
            operation.progress.current_path = progress.current_path;
        }
    }

    pub(super) fn on_archive_ready(
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
        let label = operation.kind.label();
        match result {
            Ok(count) => {
                operation.state = JobState::Done;
                operation.progress.items_done = count as u64;
                self.push_operation_log(format!("{label} completed for {count} item(s)"));
            }
            Err(_)
                if operation.control.cancel_token().is_cancelled()
                    && operation.progress.items_done == 0
                    && operation.progress.bytes_done == 0 =>
            {
                operation.state = JobState::Cancelled;
                self.push_operation_log(format!("{label} cancelled"));
            }
            Err(error) => {
                operation.state = if operation.control.cancel_token().is_cancelled() {
                    JobState::Cancelled
                } else {
                    JobState::Failed
                };
                let error = if operation.kind == OperationKind::ExtractArchive
                    && (operation.progress.items_done > 0 || operation.progress.bytes_done > 0)
                {
                    format!("{error}. Files already extracted remain in the destination.")
                } else {
                    error
                };
                operation.error = Some(error.clone());
                self.pane_mut(pane).error = Some(error.clone());
                self.push_operation_log(format!("{label}: {error}"));
            }
        }
        self.active_operations = self.active_operations.saturating_sub(1);
        self.prune_finished_operations();
        self.start_listing(PaneId::Left, sender);
        self.start_listing(PaneId::Right, sender);
    }
}
