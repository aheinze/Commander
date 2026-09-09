//! File-panel archive mutations use the existing activity and conflict UI.
use super::*;
use crate::archive::edit::Action;

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub(super) mount: Arc<archive_browser::ArchiveMount>,
    action: Action,
}

impl Request {
    pub(super) fn uses_roots(&self, roots: &[VPath]) -> bool {
        let matches = |path: &VPath| {
            roots
                .iter()
                .any(|root| path.as_path().starts_with(root.as_path()))
        };
        matches(&self.mount.source)
            || matches!(&self.action, Action::Copy { sources, .. } if sources.iter().any(matches))
    }
}

impl AppModel {
    pub(super) fn review_archive_removal(&mut self, sender: &ComponentSender<Self>) {
        let pane = self.active_pane;
        let sources = self.operation_sources(pane);
        let Some(mount) = sources
            .first()
            .and_then(|path| self.archive_mounts.mount_for(path))
        else {
            return;
        };
        if let Err(reason) = &mount.editable {
            self.pane_mut(pane).error = Some(reason.clone());
            return;
        }
        let paths = sources
            .iter()
            .map(|path| mount.entry_name(path))
            .collect::<Result<Vec<_>, _>>();
        let paths = match paths {
            Ok(paths) if !paths.is_empty() && paths.iter().all(|path| !path.is_empty()) => paths,
            _ => {
                self.pane_mut(pane).error =
                    Some("Select entries inside the archive to remove.".into());
                return;
            }
        };
        let Some(window) = relm4::main_application().active_window() else {
            return;
        };
        let count = paths.len();
        let body = format!(
            "Remove {count} selected {} from the archive? Removing a folder includes its contents. A recovery copy of the original archive will be kept.",
            if count == 1 { "item" } else { "items" }
        );
        let dialog = AlertSheet::new(Some("Remove from Archive?"), Some(&body));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("remove", "Remove");
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        let request = Request {
            mount,
            action: Action::Remove { paths },
        };
        let input = sender.input_sender().clone();
        dialog.connect_response(Some("remove"), move |_, _| {
            let _ = input.send(AppMsg::UpdateArchive {
                pane,
                request: request.clone(),
            });
        });
        dialog.present(Some(&window));
    }

    pub(super) fn archive_create(
        &mut self,
        pane: PaneId,
        path: VPath,
        directory: bool,
        sender: &ComponentSender<Self>,
    ) {
        let Some(mount) = self.archive_mounts.mount_for(&path) else {
            return;
        };
        match mount.entry_name(&path) {
            Ok(path) => self.start_archive_update(
                pane,
                Request {
                    mount,
                    action: Action::Create { path, directory },
                },
                sender,
            ),
            Err(error) => self.pane_mut(pane).error = Some(error),
        }
    }

    pub(super) fn archive_rename(
        &mut self,
        pane: PaneId,
        source: VPath,
        destination: VPath,
        sender: &ComponentSender<Self>,
    ) {
        let Some(mount) = self.archive_mounts.mount_for(&source) else {
            return;
        };
        match mount.entry_name(&source).and_then(|source| {
            mount
                .entry_name(&destination)
                .map(|destination| Action::Rename {
                    source,
                    destination,
                })
        }) {
            Ok(action) => self.start_archive_update(pane, Request { mount, action }, sender),
            Err(error) => self.pane_mut(pane).error = Some(error),
        }
    }

    pub(super) fn archive_copy(
        &mut self,
        pane: PaneId,
        sources: Vec<VPath>,
        destination: VPath,
        sender: &ComponentSender<Self>,
    ) {
        let Some(mount) = self.archive_mounts.mount_for(&destination) else {
            return;
        };
        match mount.entry_name(&destination) {
            Ok(directory) => self.start_archive_update(
                pane,
                Request {
                    mount,
                    action: Action::Copy { sources, directory },
                },
                sender,
            ),
            Err(error) => self.pane_mut(pane).error = Some(error),
        }
    }

    pub(super) fn archive_remove(
        &mut self,
        pane: PaneId,
        sources: Vec<VPath>,
        sender: &ComponentSender<Self>,
    ) {
        let Some(mount) = sources
            .first()
            .and_then(|path| self.archive_mounts.mount_for(path))
        else {
            return;
        };
        let paths = sources
            .iter()
            .map(|path| mount.entry_name(path))
            .collect::<Result<Vec<_>, _>>();
        match paths {
            Ok(paths) if paths.iter().all(|path| !path.is_empty()) => self.start_archive_update(
                pane,
                Request {
                    mount,
                    action: Action::Remove { paths },
                },
                sender,
            ),
            Ok(_) => {
                self.pane_mut(pane).error =
                    Some("Leave the archive before deleting the archive itself.".into())
            }
            Err(error) => self.pane_mut(pane).error = Some(error),
        }
    }

    pub(super) fn start_archive_update(
        &mut self,
        pane: PaneId,
        request: Request,
        sender: &ComponentSender<Self>,
    ) {
        if self.history_busy {
            self.pane_mut(pane).error =
                Some("Wait for undo or redo to finish before changing files".into());
            return;
        }
        if let Err(reason) = &request.mount.editable {
            self.pane_mut(pane).error = Some(reason.clone());
            return;
        }
        if self.operations.values().any(|operation| operation.is_active()
            && matches!(&operation.retry, OperationRetry::UpdateArchive { request: active, .. } if active.mount.source == request.mount.source)) {
            self.pane_mut(pane).error = Some("This archive is being updated. Wait for the operation to finish.".into());
            return;
        }
        let id = JobId::next();
        let control = JobControl::new();
        let (commands, responses) = std::sync::mpsc::channel();
        let source = request.mount.source.clone();
        self.operations.insert(
            id,
            OperationStatus {
                phase: JobPhase::Preparing,
                kind: OperationKind::UpdateArchive,
                state: JobState::Running,
                progress: JobProgress {
                    current_path: Some(source.clone()),
                    ..JobProgress::default()
                },
                control: control.clone(),
                commands,
                retry: OperationRetry::UpdateArchive {
                    pane,
                    request: request.clone(),
                },
                waiting_for_conflict: false,
                error: None,
            },
        );
        self.active_operations = self.active_operations.saturating_add(1);
        self.pane_mut(pane).error = None;
        let input = sender.input_sender().clone();
        let failure_source = source.clone();
        match thread::Builder::new()
            .name(format!("commander-archive-update-{}", id.get()))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut task = ArchiveTask::with_progress(&control, |progress| {
                        let _ = input.send(AppMsg::ArchiveProgress { id, progress });
                    });
                    let journal = Arc::new(
                        dualpane_engine::journal::JobJournal::create(
                            &recovery::journal_directory(),
                            JobKind::Copy,
                            vec![source.clone()],
                            Some(source.clone()),
                        )
                        .map_err(|e| e.message)?,
                    );
                    task.set_journal(journal.clone());
                    let mut remembered = None;
                    let result = crate::archive::edit::apply(
                        request.mount.editable.as_ref().map_err(Clone::clone)?,
                        request.mount.root(),
                        &request.action,
                        &mut task,
                        |conflict| {
                            if let Some(policy) = remembered {
                                return Ok(policy);
                            }
                            let conflict_id = ConflictId::next();
                            input
                                .send(AppMsg::OperationEvent {
                                    kind: JobKind::Copy,
                                    event: JobEvent::Conflict {
                                        id,
                                        conflict_id,
                                        conflict: Box::new(conflict),
                                    },
                                })
                                .map_err(|_| "The archive operation was closed".to_owned())?;
                            loop {
                                control
                                    .checkpoint()
                                    .map_err(|_| "Archive update cancelled".to_owned())?;
                                match responses.recv_timeout(Duration::from_millis(100)) {
                                    Ok(JobBridgeCommand::Resolve {
                                        conflict_id: expected,
                                        choice,
                                        apply_to_all,
                                    }) if expected == conflict_id => {
                                        let policy = match choice {
                                            ConflictChoice::Replace => ConflictPolicy::Overwrite,
                                            ConflictChoice::Skip => ConflictPolicy::Skip,
                                            ConflictChoice::KeepBoth => ConflictPolicy::Rename,
                                            ConflictChoice::ReplaceIfNewer => {
                                                ConflictPolicy::OverwriteIfNewer
                                            }
                                        };
                                        if apply_to_all {
                                            remembered = Some(policy);
                                        }
                                        break Ok(policy);
                                    }
                                    Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (),
                                    Err(_) => break Err("The archive operation was closed".into()),
                                }
                            }
                        },
                    );
                    task.flush();
                    let state = if result.is_ok() {
                        JobState::Done
                    } else if control.cancel_token().is_cancelled() {
                        JobState::Cancelled
                    } else {
                        JobState::Failed
                    };
                    journal
                        .append(&dualpane_engine::journal::JournalEvent::Finished {
                            state,
                            errors: result.as_ref().err().cloned().into_iter().collect(),
                            completed_items: u64::from(matches!(result, Ok(Some(_)))),
                        })
                        .map_err(|e| e.message)?;
                    result
                }))
                .unwrap_or_else(|_| Err("The archive update worker stopped unexpectedly".into()));
                let _ = input.send(AppMsg::ArchiveUpdateReady {
                    id,
                    pane,
                    source,
                    result,
                });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => self.on_archive_update_ready(
                id,
                pane,
                failure_source,
                Err(format!("Could not start archive update: {error}")),
                sender,
            ),
        }
    }

    pub(super) fn on_archive_update_ready(
        &mut self,
        id: JobId,
        _pane: PaneId,
        source: VPath,
        result: Result<Option<dualpane_engine::journal::ArchiveChange>, String>,
        sender: &ComponentSender<Self>,
    ) {
        let Some(operation) = self
            .operations
            .get_mut(&id)
            .filter(|operation| operation.is_active())
        else {
            return;
        };
        operation.waiting_for_conflict = false;
        let changed = match result {
            Ok(backup) => {
                operation.state = JobState::Done;
                if let Some(change) = backup {
                    self.push_operation_log(format!(
                        "Updated {source}. Recovery copy: {}",
                        change.backup
                    ));
                    self.record_history(HistoryEntry::Archive { change });
                    notifications::success("Archive updated");
                    true
                } else {
                    self.push_operation_log(format!("Archive unchanged: {source}"));
                    false
                }
            }
            Err(error) => {
                operation.state = if operation.control.cancel_token().is_cancelled() {
                    JobState::Cancelled
                } else {
                    JobState::Failed
                };
                operation.error = Some(error.clone());
                notifications::error(&error);
                self.push_operation_log(format!("Archive update: {error}"));
                // Refresh even after an error: publication may have succeeded before a
                // durability error, or another application may have replaced the source.
                true
            }
        };
        self.active_operations = self.active_operations.saturating_sub(1);
        self.prune_finished_operations();
        if changed {
            self.on_archive_edited(source, sender);
        }
    }
}
