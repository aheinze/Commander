//! Engine job orchestration: transfers, retries, clipboard pastes, and undo history.

use super::*;

impl AppModel {
    pub(super) fn start_operation(&mut self, command: CommandId, sender: &ComponentSender<Self>) {
        if self.history_busy {
            self.pane_mut(self.active_pane).error =
                Some("Wait for undo or redo to finish before changing files".to_owned());
            return;
        }
        let source_pane = self.active_pane;
        let sources = self.operation_sources(source_pane);
        if sources.is_empty() {
            self.pane_mut(source_pane).error =
                Some("No item is available to operate on".to_owned());
            return;
        }
        let destination = self.pane(source_pane.other()).current_directory().clone();
        if matches!(command, CommandId::Copy | CommandId::Move)
            && self.is_archive_browse_path(&destination)
        {
            self.pane_mut(source_pane).error =
                Some("Archive browsing is read-only; copy items out instead".to_owned());
            return;
        }
        if command != CommandId::Copy
            && sources
                .iter()
                .any(|source| self.is_archive_browse_path(source))
        {
            self.pane_mut(source_pane).error =
                Some("Archive browsing is read-only; copy items out instead".to_owned());
            return;
        }
        let confirmed_count = sources.len();
        let retry = match command {
            CommandId::Copy => OperationRetry::Copy {
                pane: source_pane,
                sources: sources.clone(),
                destination: destination.clone(),
            },
            CommandId::Move => OperationRetry::Move {
                pane: source_pane,
                sources: sources.clone(),
                destination: destination.clone(),
            },
            CommandId::Trash => OperationRetry::Trash {
                pane: source_pane,
                sources: sources.clone(),
            },
            CommandId::DeletePermanent => OperationRetry::Delete {
                pane: source_pane,
                sources: sources.clone(),
            },
            _ => return,
        };
        let history = match command {
            CommandId::Copy => Some(HistoryEntry::Copy {
                records: Vec::new(),
                trashed: Vec::new(),
            }),
            CommandId::Move => Some(HistoryEntry::Move {
                records: Vec::new(),
            }),
            _ => None,
        };
        let transfer = TransferOptions {
            conflict_policy: ConflictPolicy::Ask,
            verify: true,
            durable: true,
            parallel: self.parallel_transfers,
        };
        let (kind, handle) = match command {
            CommandId::Copy => (
                JobKind::Copy,
                self.operation_engine.spawn_copy(
                    sources,
                    destination,
                    ScanOptions::default(),
                    transfer,
                ),
            ),
            CommandId::Move => (
                JobKind::Move,
                self.operation_engine.spawn_move(
                    sources,
                    destination,
                    ScanOptions::default(),
                    transfer,
                ),
            ),
            CommandId::Trash => (JobKind::Trash, self.operation_engine.spawn_trash(sources)),
            CommandId::DeletePermanent => (
                JobKind::DeletePermanent,
                self.operation_engine
                    .spawn_delete_permanently(sources, confirmed_count),
            ),
            _ => return,
        };
        self.monitor_operation(source_pane, kind, handle, history, retry, sender);
    }

    pub(super) fn retry_operation(&mut self, id: JobId, sender: &ComponentSender<Self>) {
        let Some(operation) = self.operations.get(&id) else {
            return;
        };
        if !matches!(operation.state, JobState::Cancelled | JobState::Failed) {
            return;
        }
        let retry = operation.retry.clone();
        self.operations.remove(&id);
        let transfer = TransferOptions {
            conflict_policy: ConflictPolicy::Ask,
            verify: true,
            durable: true,
            parallel: self.parallel_transfers,
        };
        let (pane, kind, handle, history) = match &retry {
            OperationRetry::Copy {
                pane,
                sources,
                destination,
            } => (
                *pane,
                JobKind::Copy,
                self.operation_engine.spawn_copy(
                    sources.clone(),
                    destination.clone(),
                    ScanOptions::default(),
                    transfer,
                ),
                Some(HistoryEntry::Copy {
                    records: Vec::new(),
                    trashed: Vec::new(),
                }),
            ),
            OperationRetry::Move {
                pane,
                sources,
                destination,
            } => (
                *pane,
                JobKind::Move,
                self.operation_engine.spawn_move(
                    sources.clone(),
                    destination.clone(),
                    ScanOptions::default(),
                    transfer,
                ),
                Some(HistoryEntry::Move {
                    records: Vec::new(),
                }),
            ),
            OperationRetry::Trash { pane, sources } => (
                *pane,
                JobKind::Trash,
                self.operation_engine.spawn_trash(sources.clone()),
                None,
            ),
            OperationRetry::Delete { pane, sources } => (
                *pane,
                JobKind::DeletePermanent,
                self.operation_engine
                    .spawn_delete_permanently(sources.clone(), sources.len()),
                None,
            ),
            OperationRetry::Archive { .. } => {
                self.start_archive_operation(retry, sender);
                return;
            }
        };
        self.monitor_operation(pane, kind, handle, history, retry, sender);
    }

    pub(super) fn monitor_operation(
        &mut self,
        source_pane: PaneId,
        kind: JobKind,
        handle: JobHandle,
        history: Option<HistoryEntry>,
        retry: OperationRetry,
        sender: &ComponentSender<Self>,
    ) {
        self.active_operations = self.active_operations.saturating_add(1);
        let id = handle.id();
        if let Some(history) = history {
            self.pending_history.insert(id, history);
        }
        let (job_commands, job_command_receiver) = std::sync::mpsc::channel();
        self.operations.insert(
            id,
            OperationStatus {
                phase: JobPhase::Preparing,
                kind: OperationKind::Files(kind),
                state: JobState::Scanning,
                progress: JobProgress::default(),
                control: handle.control(),
                commands: job_commands,
                retry,
                waiting_for_conflict: false,
                error: None,
            },
        );
        let input = sender.input_sender().clone();
        let events = handle.events().clone();
        let vfs = Arc::clone(&self.vfs);
        match thread::Builder::new()
            .name(format!("dualpane-job-bridge-{}", handle.id().get()))
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    let finished = matches!(event, JobEvent::Finished { .. });
                    let pending_conflict = match &event {
                        JobEvent::Conflict {
                            conflict_id,
                            conflict,
                            ..
                        } => Some((*conflict_id, conflict.clone())),
                        _ => None,
                    };
                    let _ = input.send(AppMsg::OperationEvent { kind, event });
                    if let Some((expected_id, conflict)) = pending_conflict {
                        loop {
                            if handle.control().cancel_token().is_cancelled() {
                                break;
                            }
                            let command = match job_command_receiver
                                .recv_timeout(Duration::from_millis(100))
                            {
                                Ok(command) => command,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                    handle.cancel();
                                    break;
                                }
                            };
                            let JobBridgeCommand::Resolve {
                                conflict_id,
                                choice,
                                apply_to_all,
                            } = command;
                            if conflict_id != expected_id {
                                continue;
                            }
                            let action = match choice {
                                ConflictChoice::Replace => ConflictAction::Overwrite,
                                ConflictChoice::Skip => ConflictAction::Skip,
                                ConflictChoice::KeepBoth => ConflictAction::Rename(
                                    unique_renamed_path(&conflict.destination, |candidate| {
                                        vfs.stat(candidate, false).is_ok()
                                    }),
                                ),
                                ConflictChoice::ReplaceIfNewer => ConflictAction::OverwriteIfNewer,
                            };
                            let _ = handle.resolve_conflict(
                                conflict_id,
                                ConflictDecision {
                                    action,
                                    apply_to_all,
                                },
                            );
                            break;
                        }
                    }
                    if finished {
                        break;
                    }
                }
                let summary = handle.join();
                let _ = input.send(AppMsg::OperationFinished(FinishedOperation {
                    id: summary.id,
                    source_pane,
                    kind: summary.kind,
                    state: summary.state,
                    errors: summary.outcome.errors,
                    trash_records: summary.outcome.trash_records,
                    transfers: summary.outcome.transfers,
                    backups: summary.outcome.backups,
                    completed_items: summary.outcome.completed_items,
                }));
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.active_operations = self.active_operations.saturating_sub(1);
                self.operations.remove(&id);
                self.pending_history.remove(&id);
                self.pane_mut(source_pane).error =
                    Some(format!("failed to monitor operation: {error}"));
            }
        }
    }

    pub(super) fn start_drop_transfer(
        &mut self,
        mut sources: Vec<VPath>,
        destination: VPath,
        action: FileDropAction,
        sender: &ComponentSender<Self>,
    ) {
        if self.history_busy {
            self.pane_mut(self.active_pane).error =
                Some("Wait for undo or redo to finish before changing files".to_owned());
            return;
        }
        sources.sort();
        sources.dedup();
        if action == FileDropAction::Move {
            sources.retain(|source| source.parent().as_ref() != Some(&destination));
        }
        if sources.is_empty() {
            return;
        }
        if self.is_archive_browse_path(&destination)
            || (action == FileDropAction::Move
                && sources
                    .iter()
                    .any(|source| self.is_archive_browse_path(source)))
        {
            self.pane_mut(self.active_pane).error =
                Some("Archive browsing is read-only; copy items out instead".to_owned());
            return;
        }
        if let Some(source) = sources.iter().find(|source| {
            destination == **source || destination.as_path().starts_with(source.as_path())
        }) {
            self.pane_mut(self.active_pane).error = Some(format!(
                "Cannot {} {source} into itself",
                match action {
                    FileDropAction::Copy => "copy",
                    FileDropAction::Move => "move",
                }
            ));
            return;
        }
        let source_pane = [PaneId::Left, PaneId::Right]
            .into_iter()
            .find(|pane| {
                sources.iter().any(|source| {
                    source.parent().as_ref() == Some(self.pane(*pane).current_directory())
                })
            })
            .unwrap_or(self.active_pane);
        let transfer = TransferOptions {
            conflict_policy: ConflictPolicy::Ask,
            verify: true,
            durable: true,
            parallel: self.parallel_transfers,
        };
        let (kind, handle, history, retry) = match action {
            FileDropAction::Copy => {
                let history = HistoryEntry::Copy {
                    records: Vec::new(),
                    trashed: Vec::new(),
                };
                let retry = OperationRetry::Copy {
                    pane: source_pane,
                    sources: sources.clone(),
                    destination: destination.clone(),
                };
                (
                    JobKind::Copy,
                    self.operation_engine.spawn_copy(
                        sources,
                        destination,
                        ScanOptions::default(),
                        transfer,
                    ),
                    Some(history),
                    retry,
                )
            }
            FileDropAction::Move => {
                let history = common_parent(&sources).ok().map(|_| HistoryEntry::Move {
                    records: Vec::new(),
                });
                let retry = OperationRetry::Move {
                    pane: source_pane,
                    sources: sources.clone(),
                    destination: destination.clone(),
                };
                (
                    JobKind::Move,
                    self.operation_engine.spawn_move(
                        sources,
                        destination,
                        ScanOptions::default(),
                        transfer,
                    ),
                    history,
                    retry,
                )
            }
        };
        self.monitor_operation(source_pane, kind, handle, history, retry, sender);
    }

    pub(super) fn start_clipboard_transfer(
        &mut self,
        pane: PaneId,
        sources: Vec<VPath>,
        cut: bool,
        destination: VPath,
        owner: Option<u64>,
        sender: &ComponentSender<Self>,
    ) {
        if self.history_busy {
            self.pane_mut(self.active_pane).error =
                Some("Wait for undo or redo to finish before changing files".to_owned());
            return;
        }
        if self.is_archive_browse_path(&destination)
            || (cut
                && sources
                    .iter()
                    .any(|source| self.is_archive_browse_path(source)))
        {
            self.pane_mut(pane).error =
                Some("Archive browsing is read-only; copy items out instead".to_owned());
            return;
        }
        let transfer = TransferOptions {
            conflict_policy: ConflictPolicy::Ask,
            verify: true,
            durable: true,
            parallel: self.parallel_transfers,
        };
        let (kind, handle, history, retry) = if cut {
            let history = HistoryEntry::Move {
                records: Vec::new(),
            };
            let retry = OperationRetry::Move {
                pane,
                sources: sources.clone(),
                destination: destination.clone(),
            };
            (
                JobKind::Move,
                self.operation_engine.spawn_move(
                    sources,
                    destination,
                    ScanOptions::default(),
                    transfer,
                ),
                history,
                retry,
            )
        } else {
            let history = HistoryEntry::Copy {
                records: Vec::new(),
                trashed: Vec::new(),
            };
            let retry = OperationRetry::Copy {
                pane,
                sources: sources.clone(),
                destination: destination.clone(),
            };
            (
                JobKind::Copy,
                self.operation_engine.spawn_copy(
                    sources,
                    destination,
                    ScanOptions::default(),
                    transfer,
                ),
                history,
                retry,
            )
        };
        if cut && let Some(owner) = owner {
            self.clipboard_cut_jobs.insert(handle.id(), owner);
        }
        self.monitor_operation(pane, kind, handle, Some(history), retry, sender);
    }

    pub(super) fn start_batch_rename(
        &mut self,
        items: Vec<(VPath, String)>,
        sender: &ComponentSender<Self>,
    ) {
        if self.history_busy {
            self.pane_mut(self.active_pane).error =
                Some("Wait for undo or redo to finish before changing files".to_owned());
            return;
        }
        if items
            .iter()
            .any(|(path, _)| self.is_archive_browse_path(path))
        {
            self.pane_mut(self.active_pane).error =
                Some("Archive browsing is read-only; copy items out instead".to_owned());
            return;
        }
        if let Some(cancel) = self.tool_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancelToken::new();
        self.tool_cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-batch-rename".to_owned())
            .spawn(move || {
                let result = batch_rename(vfs.as_ref(), &items, &cancel);
                let _ = input.send(AppMsg::BatchRenameFinished(result));
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.tool_cancel = None;
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not start batch rename: {error}"));
            }
        }
    }

    pub(super) fn operation_sources(&self, pane: PaneId) -> Vec<VPath> {
        let state = self.pane(pane);
        if state.view_mode == PaneViewMode::Columns {
            let selected = state.selected_miller_sources();
            if !selected.is_empty() {
                return selected;
            }
            return state
                .miller_focus
                .as_ref()
                .map(|(path, _)| vec![path.clone()])
                .unwrap_or_default();
        }
        let Some(listing) = state.active().listing.as_ref() else {
            return Vec::new();
        };
        let selected: Vec<_> = listing
            .rows()
            .filter(|entry| {
                state
                    .selection
                    .contains(&SelectionKey::for_entry(listing.parent(), entry))
            })
            .map(|entry| listing.parent().join_name(entry.name()))
            .collect();
        if !selected.is_empty() {
            selected
        } else {
            listing
                .row(state.cursor_row as usize)
                .map(|entry| vec![listing.parent().join_name(entry.name())])
                .unwrap_or_default()
        }
    }

    pub(super) fn record_history(&mut self, entry: HistoryEntry) {
        const HISTORY_LIMIT: usize = 100;
        self.undo_stack.push(entry);
        if self.undo_stack.len() > HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        self.persist_history();
    }

    pub(super) fn start_history(
        &mut self,
        direction: HistoryDirection,
        sender: &ComponentSender<Self>,
    ) {
        if self.history_busy || self.active_operations > 0 {
            self.pane_mut(self.active_pane).error =
                Some("Wait for file operations to finish before undo or redo".to_owned());
            return;
        }
        let entry = match direction {
            HistoryDirection::Undo => self.undo_stack.pop(),
            HistoryDirection::Redo => self.redo_stack.pop(),
        };
        let Some(entry) = entry else {
            self.pane_mut(self.active_pane).error = Some(match direction {
                HistoryDirection::Undo => "There is nothing to undo".to_owned(),
                HistoryDirection::Redo => "There is nothing to redo".to_owned(),
            });
            return;
        };
        self.history_busy = true;
        let engine = self.operation_engine.clone();
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        let mut worker_entry = entry.clone();
        let writer = self.session_worker.history_writer();
        let pending = crate::history_store::HistoryState {
            undo: self.undo_stack.clone(),
            redo: self.redo_stack.clone(),
            pending: Some((entry.clone(), direction)),
        };
        match thread::Builder::new()
            .name("dualpane-history".to_owned())
            .spawn(move || {
                let result = writer.save_confirmed(pending).and_then(|()| {
                    apply_history(engine, vfs.as_ref(), &mut worker_entry, direction)
                });
                let _ = input.send(AppMsg::HistoryFinished {
                    entry: worker_entry,
                    direction,
                    result,
                });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.history_busy = false;
                match direction {
                    HistoryDirection::Undo => self.undo_stack.push(entry),
                    HistoryDirection::Redo => self.redo_stack.push(entry),
                }
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not start history operation: {error}"));
            }
        }
    }

    pub(super) fn on_dismiss_operation(&mut self, id: JobId) {
        if self.operations.get(&id).is_some_and(|operation| {
            matches!(
                operation.state,
                JobState::Done | JobState::Cancelled | JobState::Failed
            )
        }) {
            self.operations.remove(&id);
        }
    }

    pub(super) fn on_batch_rename_finished(
        &mut self,
        result: Result<Vec<(VPath, VPath)>, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(moves) => {
                self.tool_cancel = None;
                for (from, to) in &moves {
                    if let Some(color) = self.tags.remove(&from.to_string()) {
                        self.tags.insert(to.to_string(), color);
                    }
                }
                if !moves.is_empty() {
                    self.record_history(HistoryEntry::BatchRename { moves });
                    self.tags_revision = self.tags_revision.wrapping_add(1);
                    self.persist_session();
                }
                self.start_listing(PaneId::Left, sender);
                self.start_listing(PaneId::Right, sender);
            }
            Err(error) => {
                self.tool_cancel = None;
                self.pane_mut(self.active_pane).error = Some(error);
            }
        }
    }

    pub(super) fn on_operation_event(
        &mut self,
        kind: JobKind,
        event: JobEvent,
        sender: &ComponentSender<Self>,
    ) {
        match event {
            JobEvent::Phase { id, phase, path } => {
                if let Some(operation) = self.operations.get_mut(&id) {
                    operation.phase = phase;
                    if path.is_some() {
                        operation.progress.current_path = path;
                    }
                }
            }
            JobEvent::State { id, state } => {
                if let Some(operation) = self.operations.get_mut(&id) {
                    operation.state = state;
                }
            }
            JobEvent::Progress { id, progress } => {
                if let Some(operation) = self.operations.get_mut(&id) {
                    operation.progress = progress;
                }
            }
            JobEvent::Conflict {
                id,
                conflict_id,
                conflict,
            } => {
                if let Some(operation) = self.operations.get_mut(&id) {
                    operation.waiting_for_conflict = true;
                }
                self.push_operation_log(format!(
                    "{kind:?} job {} needs a decision for {}",
                    id.get(),
                    conflict.destination
                ));
                show_conflict_dialog(self.active_pane, id, conflict_id, &conflict, sender);
            }
            JobEvent::Finished { id, state, errors } => {
                if let Some(operation) = self.operations.get_mut(&id) {
                    operation.state = state;
                    operation.waiting_for_conflict = false;
                    operation.error = errors.first().map(|error| error.message.clone());
                }
                self.push_operation_log(format!(
                    "{kind:?} job {} finished as {state:?} with {} error(s)",
                    id.get(),
                    errors.len()
                ));
            }
        }
    }

    pub(super) fn on_resolve_conflict(
        &mut self,
        job_id: JobId,
        conflict_id: ConflictId,
        choice: ConflictChoice,
        apply_to_all: bool,
    ) {
        if let Some(operation) = self.operations.get_mut(&job_id) {
            operation.waiting_for_conflict = false;
            let _ = operation.commands.send(JobBridgeCommand::Resolve {
                conflict_id,
                choice,
                apply_to_all,
            });
        }
    }

    pub(super) fn on_compare_conflict_checksum(
        &mut self,
        pane: PaneId,
        job_id: JobId,
        conflict_id: ConflictId,
        source: VPath,
        destination: VPath,
        sender: &ComponentSender<Self>,
    ) {
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-conflict-checksum".to_owned())
            .spawn(move || {
                let cancel = CancelToken::new();
                let result = sha256(vfs.as_ref(), &source, &cancel).and_then(|left| {
                    sha256(vfs.as_ref(), &destination, &cancel).map(|right| left == right)
                });
                match result {
                    Ok(equal) => {
                        let _ = input.send(AppMsg::ResolveConflict {
                            job_id,
                            conflict_id,
                            choice: if equal {
                                ConflictChoice::Skip
                            } else {
                                ConflictChoice::Replace
                            },
                            apply_to_all: false,
                        });
                    }
                    Err(error) => {
                        let _ = input.send(AppMsg::OpenFailed(
                            pane,
                            format!("Could not compare conflicting files: {error}"),
                        ));
                        let _ = input.send(AppMsg::ResolveConflict {
                            job_id,
                            conflict_id,
                            choice: ConflictChoice::Skip,
                            apply_to_all: false,
                        });
                    }
                }
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.pane_mut(pane).error =
                    Some(format!("Could not start checksum comparison: {error}"));
                let _ = sender.input_sender().send(AppMsg::ResolveConflict {
                    job_id,
                    conflict_id,
                    choice: ConflictChoice::Skip,
                    apply_to_all: false,
                });
            }
        }
    }

    pub(super) fn on_toggle_pause_first_operation(&mut self) {
        if let Some(id) = job_view::first_active_operation(&self.operations) {
            self.on_toggle_pause_operation(id);
        }
    }

    pub(super) fn on_toggle_pause_operation(&mut self, id: JobId) {
        if let Some(operation) = self.operations.get_mut(&id)
            && operation.can_control()
            && !operation.waiting_for_conflict
        {
            if operation.control.is_paused() {
                operation.control.resume();
                if operation.state == JobState::Paused {
                    operation.state = JobState::Running;
                }
            } else {
                operation.control.pause();
            }
        }
    }

    pub(super) fn on_operation_finished(
        &mut self,
        finished: FinishedOperation,
        sender: &ComponentSender<Self>,
    ) {
        let FinishedOperation {
            backups,
            completed_items,
            id,
            source_pane,
            kind,
            state,
            errors,
            trash_records,
            transfers,
        } = finished;
        self.active_operations = self.active_operations.saturating_sub(1);
        if let Some(operation) = self.operations.get_mut(&id) {
            operation.state = state;
            operation.waiting_for_conflict = false;
            operation.error = errors.first().map(|error| error.message.clone());
        }
        if let Some(owner) = self.clipboard_cut_jobs.remove(&id)
            && let Some(display) = gdk::Display::default()
            && owner == self.clipboard_generation
            && self.clipboard_provider.is_some()
            && display.clipboard().content() == self.clipboard_provider
            && let Some(OperationStatus {
                retry: OperationRetry::Move { sources, .. },
                ..
            }) = self.operations.get(&id)
        {
            let remaining = clipboard::remaining_cut_sources(sources, &transfers);
            if remaining.is_empty() {
                let _ = display
                    .clipboard()
                    .set_content(None::<&gdk::ContentProvider>);
            } else if remaining.len() != sources.len()
                && let Some(provider) = clipboard::provider(&remaining, true)
                && display.clipboard().set_content(Some(&provider)).is_ok()
            {
                self.clipboard_generation = self.clipboard_generation.wrapping_add(1);
                self.clipboard_provider = Some(provider);
            }
        }
        let history = self.pending_history.remove(&id);
        if state == JobState::Done && errors.is_empty() {
            self.clear_selection(source_pane);
            if kind == JobKind::Trash && !trash_records.is_empty() {
                self.record_history(HistoryEntry::Trash {
                    records: trash_records,
                });
            } else if history.is_some() && !transfers.is_empty() {
                if let Some(history) =
                    fileops::transfer_history_with_backups(kind, transfers, backups)
                {
                    self.record_history(history);
                } else {
                    self.undo_stack.clear();
                    self.redo_stack.clear();
                    self.persist_history();
                    self.push_operation_log(
                        "Undo is unavailable for transfers that replace or merge existing items"
                            .to_owned(),
                    );
                }
            }
        } else {
            let summary = errors.first().map_or_else(
                || format!("{kind:?} ended in {state:?}"),
                |error| {
                    format!(
                        "{kind:?} completed with {} error(s): {}",
                        errors.len(),
                        error.message
                    )
                },
            );
            self.pane_mut(source_pane).error = Some(format!(
                "{summary}. {completed_items} item(s) completed; open Recovery for recorded destinations and retained originals."
            ));
        }
        self.prune_finished_operations();
        self.start_listing(PaneId::Left, sender);
        self.start_listing(PaneId::Right, sender);
    }

    pub(super) fn prune_finished_operations(&mut self) {
        let finished_ids = self
            .operations
            .iter()
            .filter_map(|(id, operation)| {
                matches!(
                    operation.state,
                    JobState::Done | JobState::Cancelled | JobState::Failed
                )
                .then_some(*id)
            })
            .collect::<Vec<_>>();
        for old_id in finished_ids
            .iter()
            .take(finished_ids.len().saturating_sub(20))
        {
            self.operations.remove(old_id);
        }
    }

    pub(super) fn on_history_finished(
        &mut self,
        entry: HistoryEntry,
        direction: HistoryDirection,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        self.history_busy = false;
        match result {
            Ok(()) => {
                if let HistoryEntry::BatchRename { moves } = &entry {
                    for (from, to) in moves {
                        let (source, destination) = match direction {
                            HistoryDirection::Undo => (to, from),
                            HistoryDirection::Redo => (from, to),
                        };
                        if let Some(color) = self.tags.remove(&source.to_string()) {
                            self.tags.insert(destination.to_string(), color);
                        }
                    }
                    self.tags_revision = self.tags_revision.wrapping_add(1);
                    self.persist_session();
                } else if let HistoryEntry::Rename { from, to } = &entry {
                    let (source, destination) = match direction {
                        HistoryDirection::Undo => (to, from),
                        HistoryDirection::Redo => (from, to),
                    };
                    if let Some(color) = self.tags.remove(&source.to_string()) {
                        self.tags.insert(destination.to_string(), color);
                        self.tags_revision = self.tags_revision.wrapping_add(1);
                        self.persist_session();
                    }
                }
                match direction {
                    HistoryDirection::Undo => self.redo_stack.push(entry),
                    HistoryDirection::Redo => self.undo_stack.push(entry),
                }
            }
            Err(error) => {
                match direction {
                    HistoryDirection::Undo => self.undo_stack.push(entry),
                    HistoryDirection::Redo => self.redo_stack.push(entry),
                }
                self.pane_mut(self.active_pane).error = Some(error);
            }
        }
        self.persist_history();
        self.start_listing(PaneId::Left, sender);
        self.start_listing(PaneId::Right, sender);
    }
}

impl AppModel {
    fn persist_history(&self) {
        if !self.history_busy {
            self.session_worker
                .history_writer()
                .save(crate::history_store::HistoryState {
                    undo: self.undo_stack.clone(),
                    redo: self.redo_stack.clone(),
                    pending: None,
                });
        }
    }
}
