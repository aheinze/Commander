//! Directory navigation: listings, tabs, Miller columns, archive browsing, and item creation.

use super::*;

impl AppModel {
    pub(super) fn create_directory(&mut self, name: String, sender: &ComponentSender<Self>) {
        if self.history_busy {
            self.pane_mut(self.active_pane).error =
                Some("Wait for undo or redo to finish before changing files".to_owned());
            return;
        }
        if self.is_archive_browse_path(self.pane(self.active_pane).current_directory()) {
            self.pane_mut(self.active_pane).error =
                Some("Archive browsing is read-only; copy items out instead".to_owned());
            return;
        }
        let name = name.trim();
        if !valid_file_name(name) {
            self.pane_mut(self.active_pane).error =
                Some("Folder name must be a single valid file name".to_owned());
            return;
        }
        let pane = self.active_pane;
        let path = self
            .pane(pane)
            .current_directory()
            .join_name(OsStr::new(name));
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        let worker = thread::Builder::new()
            .name("dualpane-create-directory".to_owned())
            .spawn(move || {
                let result = vfs
                    .create_dir(&path)
                    .map_err(|error| format!("Could not create folder {path}: {error}"));
                let _ = input.send(AppMsg::DirectoryCreated { pane, result });
            });
        match worker {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.pane_mut(pane).error =
                    Some(format!("Could not start folder creation: {error}"));
            }
        }
    }

    pub(super) fn create_file(&mut self, name: String, sender: &ComponentSender<Self>) {
        if self.history_busy {
            self.pane_mut(self.active_pane).error =
                Some("Wait for undo or redo to finish before changing files".to_owned());
            return;
        }
        if self.is_archive_browse_path(self.pane(self.active_pane).current_directory()) {
            self.pane_mut(self.active_pane).error =
                Some("Archive browsing is read-only; copy items out instead".to_owned());
            return;
        }
        let name = name.trim();
        if !valid_file_name(name) {
            self.pane_mut(self.active_pane).error =
                Some("File name must be a single valid name".to_owned());
            return;
        }
        let pane = self.active_pane;
        let path = self
            .pane(pane)
            .current_directory()
            .join_name(OsStr::new(name));
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        let worker = thread::Builder::new()
            .name("dualpane-create-file".to_owned())
            .spawn(move || {
                let result = vfs
                    .create_write(&path, SizeHint::Exact(0))
                    .map(drop)
                    .map_err(|error| format!("Could not create file {path}: {error}"));
                let _ = input.send(AppMsg::FileCreated { pane, result });
            });
        match worker {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.pane_mut(pane).error = Some(format!("Could not start file creation: {error}"));
            }
        }
    }

    pub(super) fn rename_path(
        &mut self,
        source: VPath,
        name: String,
        sender: &ComponentSender<Self>,
    ) {
        if self.history_busy {
            self.pane_mut(self.active_pane).error =
                Some("Wait for undo or redo to finish before changing files".to_owned());
            return;
        }
        if self.is_archive_browse_path(&source) {
            self.pane_mut(self.active_pane).error =
                Some("Archive browsing is read-only; copy items out instead".to_owned());
            return;
        }
        let name = name.trim();
        if !valid_file_name(name) {
            self.pane_mut(self.active_pane).error =
                Some("New name must be a single valid file name".to_owned());
            return;
        }
        let pane = self.active_pane;
        let Some(parent) = source.parent() else {
            self.pane_mut(pane).error = Some("The selected item cannot be renamed".to_owned());
            return;
        };
        let destination = parent.join_name(OsStr::new(name));
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        let worker = thread::Builder::new()
            .name("dualpane-rename".to_owned())
            .spawn(move || {
                let result = if source == destination {
                    Ok(())
                } else {
                    vfs.rename_noreplace(&source, &destination)
                        .map_err(|error| {
                            format!("Could not rename {source} to {destination}: {error}")
                        })
                };
                let _ = input.send(AppMsg::RenameFinished {
                    pane,
                    source,
                    destination,
                    result,
                });
            });
        match worker {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.pane_mut(pane).error = Some(format!("Could not start rename: {error}"));
            }
        }
    }

    pub(super) fn start_listing(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        self.sync_directory_watches(sender);
        if self.pane(pane).view_mode == PaneViewMode::Columns
            && (self.pane(pane).miller_columns.len() > 1
                || self
                    .pane(pane)
                    .miller_columns
                    .first()
                    .is_some_and(|column| column.listing.is_some()))
        {
            self.refresh_miller(pane, true, sender);
            return;
        }
        let (path, sort, generation) = {
            let state = self.pane_mut(pane);
            state.cancel_work();
            state.reap_workers();
            state.thumbnail_cancel = CancelToken::new();
            state.generation = state.generation.wrapping_add(1);
            state.loading = true;
            state.error = None;
            state.active_mut().base_listing = None;
            state.active_mut().listing = None;
            state.revision = state.revision.wrapping_add(1);
            (state.active().path.clone(), state.sort, state.generation)
        };
        if pane == self.active_pane {
            self.preview_state.cancel();
            self.preview_state.path = None;
            self.preview_state.content = None;
            self.preview_state.error = None;
        }
        let task = match ListingTask::spawn(Arc::clone(&self.vfs), ListingRequest { path, sort }) {
            Ok(task) => task,
            Err(error) => {
                let state = self.pane_mut(pane);
                state.loading = false;
                state.error = Some(error.to_string());
                return;
            }
        };
        self.pane_mut(pane).listing_cancel = Some(task.cancel_token());
        let (presentation_ack, presented) = sync_channel(1);
        self.pane_mut(pane).presentation_ack = Some(presentation_ack);
        let input = sender.input_sender().clone();
        let worker = thread::Builder::new()
            .name(format!("dualpane-{}-bridge", pane.label().to_lowercase()))
            .spawn(move || {
                let mut first_publication = true;
                while let Ok(mut event) = task.receiver().recv() {
                    if !first_publication {
                        for newer in task.receiver().try_iter() {
                            event = newer;
                            if matches!(
                                &event,
                                ListingEvent::Complete { .. }
                                    | ListingEvent::Cancelled
                                    | ListingEvent::Failed(_)
                            ) {
                                break;
                            }
                        }
                    }
                    let terminal = matches!(
                        &event,
                        ListingEvent::Complete { .. }
                            | ListingEvent::Cancelled
                            | ListingEvent::Failed(_)
                    );
                    if input
                        .send(AppMsg::Listing {
                            pane,
                            generation,
                            event,
                        })
                        .is_err()
                    {
                        task.cancel();
                        break;
                    }
                    if terminal {
                        break;
                    }
                    first_publication = false;
                    let _ = presented.recv_timeout(PRESENTATION_ACK_TIMEOUT);
                }
                let _ = task.join();
            });
        match worker {
            Ok(worker) => self.pane_mut(pane).workers.push(worker),
            Err(error) => {
                if let Some(cancel) = self.pane_mut(pane).listing_cancel.take() {
                    cancel.cancel();
                }
                self.pane_mut(pane).error = Some(format!("failed to bridge listing: {error}"));
            }
        }
    }

    pub(super) fn navigate(&mut self, pane: PaneId, path: VPath, sender: &ComponentSender<Self>) {
        self.navigate_to(pane, path, None, sender);
    }

    pub(super) fn reveal_path(
        &mut self,
        pane: PaneId,
        path: VPath,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(parent) = path.parent() {
            self.navigate_to(pane, parent, Some(path), sender);
        } else {
            self.navigate(pane, path, sender);
        }
    }

    fn navigate_to(
        &mut self,
        pane: PaneId,
        path: VPath,
        reveal: Option<VPath>,
        sender: &ComponentSender<Self>,
    ) {
        self.active_pane = pane;
        self.focus_active_files();
        self.record_recent(path.clone());
        self.pane_mut(pane).remember_navigation();
        self.pane_mut(pane).active_mut().navigate(path);
        self.pane_mut(pane).reset_directory_view();
        if let Some(target) = reveal {
            let state = self.pane_mut(pane);
            // A search hit takes priority over the folder's saved cursor and branch.
            state.restore_cursor = None;
            state.restore_names.clear();
            state.miller_columns.truncate(1);
            if let Some(column) = state.miller_columns.first_mut() {
                column.restore_name = None;
                column.selected_row = None;
            }
            if target
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
            {
                state.show_hidden = true;
            }
            state.pending_reveal = Some(target);
        }
        self.pane_mut(pane).tabs_revision = self.pane(pane).tabs_revision.wrapping_add(1);
        self.start_listing(pane, sender);
        self.persist_session();
    }

    pub(super) fn close_tab(&mut self, pane: PaneId, tab: usize, sender: &ComponentSender<Self>) {
        let closing_active = {
            let state = self.pane_mut(pane);
            if state.tabs.len() <= 1 || tab >= state.tabs.len() {
                return;
            }

            let closing_active = tab == state.active_tab;
            if closing_active {
                state.remember_navigation();
                state.cancel_work();
            }
            state.tabs.remove(tab);
            if closing_active {
                state.active_tab = tab.min(state.tabs.len() - 1);
                state.reset_directory_view();
            } else if tab < state.active_tab {
                state.active_tab -= 1;
            }
            state.tabs_revision = state.tabs_revision.wrapping_add(1);
            closing_active
        };

        if closing_active {
            self.start_listing(pane, sender);
        }
        self.persist_session();
    }

    pub(super) fn record_recent(&mut self, path: VPath) {
        const RECENT_LIMIT: usize = 12;
        self.recent.retain(|candidate| candidate != &path);
        self.recent.insert(0, path);
        self.recent.truncate(RECENT_LIMIT);
    }

    pub(super) fn open_row(&mut self, pane: PaneId, row: u32, sender: &ComponentSender<Self>) {
        let target = self
            .pane(pane)
            .active()
            .listing
            .as_ref()
            .and_then(|listing| listing.row(row as usize))
            .map(|entry| {
                (
                    entry.kind(),
                    self.pane(pane).active().path.join_name(entry.name()),
                )
            });
        let Some((kind, target)) = target else {
            return;
        };
        self.open_path(pane, kind, target, sender);
    }

    pub(super) fn open_path(
        &mut self,
        pane: PaneId,
        kind: EntryKind,
        target: VPath,
        sender: &ComponentSender<Self>,
    ) {
        if kind == EntryKind::Directory {
            self.navigate(pane, target, sender);
            return;
        }
        if is_archive_path(&target) {
            self.start_archive_browse(pane, target, sender);
            return;
        }
        let uri = gio::File::for_path(target.as_path()).uri();
        let input = sender.input_sender().clone();
        gio::AppInfo::launch_default_for_uri_async(
            &uri,
            None::<&gio::AppLaunchContext>,
            None::<&gio::Cancellable>,
            move |result| {
                if let Err(error) = result {
                    let _ = input.send(AppMsg::OpenFailed(
                        pane,
                        format!("Could not open the selected item: {error}"),
                    ));
                }
            },
        );
    }

    pub(super) fn start_archive_browse(
        &mut self,
        pane: PaneId,
        source: VPath,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(cancel) = self.tool_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancelToken::new();
        self.tool_cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        let worker_source = source.clone();
        match thread::Builder::new()
            .name("dualpane-archive-browser".to_owned())
            .spawn(move || {
                let result = tempfile::Builder::new()
                    .prefix("omacommander-archive-")
                    .tempdir()
                    .map_err(|error| error.to_string())
                    .and_then(|directory| {
                        let destination = VPath::from(directory.path());
                        extract_archive(
                            vfs.as_ref(),
                            &worker_source,
                            &destination,
                            &mut ArchiveTask::new(&cancel),
                        )?;
                        Ok(directory)
                    });
                let _ = input.send(AppMsg::ArchiveBrowseReady {
                    pane,
                    source,
                    result,
                });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.tool_cancel = None;
                self.pane_mut(pane).error =
                    Some(format!("Could not start archive browser: {error}"));
            }
        }
    }

    pub(super) fn is_archive_browse_path(&self, path: &VPath) -> bool {
        self.archive_mounts
            .iter()
            .any(|(_, directory)| path.as_path().starts_with(directory.path()))
    }

    pub(super) fn sync_miller_root(&mut self, pane: PaneId) {
        if self.pane(pane).view_mode != PaneViewMode::Columns {
            return;
        }
        let path = self.pane(pane).active().path.clone();
        let listing = self.pane(pane).active().listing.clone();
        let state = self.pane_mut(pane);
        if let Some(root) = state.miller_columns.first_mut()
            && root.path == path
        {
            // Keep the visible branch intact until a refresh has a complete snapshot.
            // A selected folder may not be present in an early listing batch yet.
            if state.loading && root.listing.is_some() {
                root.loading = true;
                root.error.clone_from(&state.error);
                state.miller_revision = state.miller_revision.wrapping_add(1);
                return;
            }
            let selected_name = root.restore_name.clone().or_else(|| {
                root.selected_row
                    .and_then(|row| root.listing.as_ref()?.row(row as usize))
                    .map(|entry| entry.name().to_os_string())
            });
            if !state.loading {
                root.restore_name = None;
            }
            root.selected_row = listing.as_ref().and_then(|listing| {
                selected_name
                    .as_ref()
                    .and_then(|name| {
                        listing
                            .rows()
                            .position(|entry| entry.name() == name)
                            .map(|row| row as u32)
                    })
                    .or_else(|| (!listing.is_empty()).then_some(0))
            });
            root.base_listing = state.tabs[state.active_tab].base_listing.clone();
            root.listing = listing;
            root.loading = state.loading;
            root.error.clone_from(&state.error);
        } else {
            state.miller_columns.clear();
            state.miller_columns.push(MillerColumnState {
                base_listing: state.active().base_listing.clone(),
                restore_name: None,
                scroll_y: 0,
                path,
                listing,
                selected_row: None,
                loading: state.loading,
                error: state.error.clone(),
                width: 280,
            });
            state.miller_focus = None;
        }
        let root_focus = state.miller_columns.first().and_then(|root| {
            let entry = root.listing.as_ref()?.row(root.selected_row? as usize)?;
            Some((root.path.join_name(entry.name()), entry.kind()))
        });
        if state.miller_columns.get(1).is_some_and(|child| {
            root_focus
                .as_ref()
                .is_none_or(|(path, _)| path != &child.path)
        }) {
            if let Some(cancel) = state.miller_cancel.take() {
                cancel.cancel();
            }
            state.miller_generation = state.miller_generation.wrapping_add(1);
            state.miller_columns.truncate(1);
            state.selection.clear();
            state.selection_revision = state.selection_revision.wrapping_add(1);
        }
        if state.miller_columns.len() == 1 {
            state.miller_focus = root_focus;
        }
        if state.miller_columns.len() == 1
            && state.miller_focus.is_none()
            && let Some(root) = state.miller_columns.first_mut()
            && let Some(listing) = root.listing.as_ref()
            && let Some(entry) = listing.row(0)
        {
            root.selected_row = Some(0);
            state.miller_focus = Some((root.path.join_name(entry.name()), entry.kind()));
        }
        state.miller_revision = state.miller_revision.wrapping_add(1);
    }

    pub(super) fn open_miller_row(
        &mut self,
        pane: PaneId,
        column: usize,
        row: u32,
        sender: &ComponentSender<Self>,
    ) {
        let target = self
            .pane(pane)
            .miller_columns
            .get(column)
            .and_then(|column| column.listing.as_ref())
            .and_then(|listing| listing.row(row as usize))
            .map(|entry| {
                let parent = &self.pane(pane).miller_columns[column].path;
                (
                    parent.join_name(entry.name()),
                    entry.kind(),
                    SelectionKey::for_entry(parent, entry),
                )
            });
        let Some((path, kind, key)) = target else {
            return;
        };
        self.active_pane = pane;
        self.pane_mut(pane).remember_navigation();
        let next_width = self
            .pane(pane)
            .miller_columns
            .get(column + 1)
            .filter(|next| next.path == path)
            .map_or_else(
                || {
                    self.pane(pane)
                        .folder_views
                        .get(&path.to_string())
                        .map_or(280, |saved| saved.column_width.clamp(220, 600))
                },
                |next| next.width,
            );
        // Re-activating an already open folder must not discard its scroll position.
        if kind == EntryKind::Directory
            && self
                .pane(pane)
                .miller_columns
                .get(column + 1)
                .is_some_and(|next| next.path == path && next.error.is_none())
        {
            let state = self.pane_mut(pane);
            state.selection.replace([key]);
            state.selection_revision = state.selection_revision.wrapping_add(1);
            state.restore_names.clear();
            state.range_anchor = None;
            state.miller_focus = Some((path, kind));
            state.miller_revision = state.miller_revision.wrapping_add(1);
            return;
        }
        let (generation, next_column, sort, show_hidden) = {
            let state = self.pane_mut(pane);
            if column + 1 < state.miller_columns.len() || kind == EntryKind::Directory {
                state.selection.replace([key]);
                state.selection_revision = state.selection_revision.wrapping_add(1);
                state.restore_names.clear();
                state.range_anchor = None;
            }
            if let Some(cancel) = state.miller_cancel.take() {
                cancel.cancel();
            }
            state.miller_columns.truncate(column + 1);
            if let Some(current) = state.miller_columns.get_mut(column) {
                current.selected_row = Some(row);
            }
            state.miller_focus = Some((path.clone(), kind));
            state.miller_generation = state.miller_generation.wrapping_add(1);
            let generation = state.miller_generation;
            if kind == EntryKind::Directory {
                state.miller_columns.push(MillerColumnState {
                    base_listing: None,
                    restore_name: state
                        .folder_views
                        .get(&path.to_string())
                        .and_then(|saved| saved.cursor_name.as_ref())
                        .map(OsString::from),
                    scroll_y: state
                        .folder_views
                        .get(&path.to_string())
                        .map_or(0, |saved| saved.scroll_y),
                    path: path.clone(),
                    listing: None,
                    selected_row: None,
                    loading: true,
                    error: None,
                    width: next_width,
                });
            }
            state.miller_revision = state.miller_revision.wrapping_add(1);
            (generation, column + 1, state.sort, state.show_hidden)
        };
        self.start_preview(sender);
        if kind != EntryKind::Directory {
            return;
        }
        let _ = (generation, next_column, sort, show_hidden);
        self.pane_mut(pane).filter_query.clear();
        self.refresh_miller(pane, false, sender);
        self.persist_session();
    }

    pub(super) fn handle_listing(
        &mut self,
        pane: PaneId,
        generation: u64,
        event: ListingEvent,
        sender: &ComponentSender<Self>,
    ) {
        if generation != self.pane(pane).generation {
            return;
        }
        let shared_complete = match &event {
            ListingEvent::Complete { listing, .. } => self
                .pane(pane.other())
                .active()
                .base_listing
                .as_ref()
                .filter(|other| other.has_same_rows_as(listing))
                .cloned(),
            ListingEvent::Snapshot { .. } | ListingEvent::Cancelled | ListingEvent::Failed(_) => {
                None
            }
        };
        let state = self.pane_mut(pane);
        match event {
            ListingEvent::Snapshot { listing, .. } => {
                state.active_mut().base_listing = Some(Arc::clone(&listing));
                state.active_mut().listing =
                    Some(if state.filter_query.is_empty() && !state.show_hidden {
                        listing.without_hidden()
                    } else {
                        listing
                    });
                state.revision = state.revision.wrapping_add(1);
            }
            ListingEvent::Complete { listing, timings } => {
                let listing = shared_complete.unwrap_or(listing);
                tracing::info!(
                    pane = pane.label(),
                    entries = listing.len(),
                    first_snapshot_ms = timings
                        .first_snapshot
                        .map_or(0.0, |value| value.as_secs_f64() * 1_000.0),
                    total_ms = timings.total.as_secs_f64() * 1_000.0,
                    "directory listing complete"
                );
                if !state.selection.is_empty() {
                    let available = listing
                        .rows()
                        .map(|entry| SelectionKey::for_entry(listing.parent(), entry))
                        .collect();
                    let previous = state.selection.len();
                    state.selection.retain_available(&available);
                    if state.selection.len() != previous {
                        state.selection_revision = state.selection_revision.wrapping_add(1);
                    }
                }
                state.active_mut().base_listing = Some(Arc::clone(&listing));
                state.active_mut().listing =
                    Some(if state.filter_query.is_empty() && !state.show_hidden {
                        listing.without_hidden()
                    } else {
                        listing
                    });
                state.loading = false;
                state.listing_cancel = None;
                state.revision = state.revision.wrapping_add(1);
            }
            ListingEvent::Cancelled => {
                state.loading = false;
                state.listing_cancel = None;
            }
            ListingEvent::Failed(error) => {
                state.loading = false;
                state.listing_cancel = None;
                state.error = Some(error.to_string());
            }
        }
        state.restore_selection();
        let should_filter = state
            .active()
            .listing
            .as_ref()
            .is_some_and(|listing| listing.is_complete() && !state.filter_query.is_empty());
        let listing_complete = state
            .active()
            .base_listing
            .as_ref()
            .is_some_and(|listing| listing.is_complete());
        let _ = state;
        if listing_complete && pane == self.active_pane {
            self.ensure_filter_worker(pane, sender);
        }
        if should_filter {
            self.start_filter(pane, sender);
        }
        self.sync_miller_root(pane);
        self.pane_mut(pane).reveal_pending_item();
        if pane == self.active_pane {
            self.start_preview(sender);
        }
    }

    pub(super) fn on_rename_finished(
        &mut self,
        pane: PaneId,
        source: VPath,
        destination: VPath,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(()) => {
                if let Some(color) = self.tags.remove(&source.to_string()) {
                    self.tags.insert(destination.to_string(), color);
                    self.tags_revision = self.tags_revision.wrapping_add(1);
                    self.persist_session();
                }
                self.record_history(HistoryEntry::Rename {
                    from: source,
                    to: destination,
                });
                self.start_listing(pane, sender);
            }
            Err(error) => self.pane_mut(pane).error = Some(error),
        }
    }

    pub(super) fn on_open_cursor(&mut self, sender: &ComponentSender<Self>) {
        let pane = self.active_pane;
        if self.pane(pane).view_mode == PaneViewMode::Columns {
            // Column view keeps its context: a folder opens in the next column, the
            // same as Right, rather than re-rooting the pane. Files open as usual.
            let state = self.pane(pane);
            let column = state.miller_columns.len().saturating_sub(1);
            let next_column = state
                .miller_columns
                .get(column)
                .and_then(|column| column.selected_row)
                .filter(|_| {
                    state
                        .miller_focus
                        .as_ref()
                        .is_some_and(|(_, kind)| *kind == EntryKind::Directory)
                });
            if let Some(row) = next_column {
                self.open_miller_row(pane, column, row, sender);
            } else if let Some((path, kind)) = state.miller_focus.clone() {
                self.open_path(pane, kind, path, sender);
            }
        } else {
            let row = self.pane(pane).cursor_row;
            self.open_row(pane, row, sender);
        }
    }

    pub(super) fn on_back(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        self.pane_mut(pane).remember_navigation();
        if self.pane_mut(pane).active_mut().back() {
            self.pane_mut(pane).reset_directory_view();
            self.start_listing(pane, sender);
            self.persist_session();
        }
    }

    pub(super) fn on_forward(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        self.pane_mut(pane).remember_navigation();
        if self.pane_mut(pane).active_mut().forward() {
            self.pane_mut(pane).reset_directory_view();
            self.start_listing(pane, sender);
            self.persist_session();
        }
    }

    pub(super) fn on_new_tab(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        let path = self.pane(pane).current_directory().clone();
        let state = self.pane_mut(pane);
        state.remember_navigation();
        state.tabs.push(TabState::new(path));
        state.active_tab = state.tabs.len() - 1;
        state.reset_directory_view();
        state.tabs_revision = state.tabs_revision.wrapping_add(1);
        self.start_listing(pane, sender);
        self.persist_session();
    }

    pub(super) fn on_select_tab(
        &mut self,
        pane: PaneId,
        tab: usize,
        sender: &ComponentSender<Self>,
    ) {
        let state = self.pane_mut(pane);
        if tab < state.tabs.len() && tab != state.active_tab {
            state.cancel_work();
            state.remember_navigation();
            state.active_tab = tab;
            state.reset_directory_view();
            state.tabs_revision = state.tabs_revision.wrapping_add(1);
            state.revision = state.revision.wrapping_add(1);
            self.start_listing(pane, sender);
            self.persist_session();
        }
    }

    pub(super) fn on_save_workspace(&mut self, name: String) {
        let name = name.trim();
        if !name.is_empty() {
            let snapshot = self.workspace_snapshot(name);
            if let Some(index) = self
                .workspaces
                .iter()
                .position(|workspace| workspace.name.eq_ignore_ascii_case(name))
            {
                self.workspaces[index] = snapshot;
            } else {
                self.workspaces.push(snapshot);
            }
            self.persist_session();
        }
    }

    pub(super) fn on_update_workspace(&mut self, index: usize) {
        if let Some(name) = self
            .workspaces
            .get(index)
            .map(|workspace| workspace.name.clone())
        {
            self.workspaces[index] = self.workspace_snapshot(&name);
            self.persist_session();
        }
    }

    pub(super) fn on_rename_workspace(&mut self, index: usize, name: String) {
        let name = name.trim();
        if name.is_empty() || index >= self.workspaces.len() {
            return;
        }
        if self
            .workspaces
            .iter()
            .enumerate()
            .any(|(other, workspace)| other != index && workspace.name.eq_ignore_ascii_case(name))
        {
            self.pane_mut(self.active_pane).error =
                Some(format!("A workspace named “{name}” already exists"));
        } else {
            self.workspaces[index].name = name.to_owned();
            self.persist_session();
        }
    }

    pub(super) fn on_open_workspace(&mut self, index: usize, sender: &ComponentSender<Self>) {
        if let Some(workspace) = self.workspaces.get(index).cloned() {
            let legacy_pane = |path: &str| PaneSession {
                tabs: vec![path.to_owned()],
                active_tab: 0,
                ..PaneSession::default()
            };
            let left = workspace
                .left_pane
                .unwrap_or_else(|| legacy_pane(&workspace.left));
            let right = workspace
                .right_pane
                .unwrap_or_else(|| legacy_pane(&workspace.right));
            self.pane_mut(PaneId::Left)
                .apply_session(&left, VPath::from(workspace.left.as_str()));
            self.pane_mut(PaneId::Right)
                .apply_session(&right, VPath::from(workspace.right.as_str()));
            if let Some(dual_pane) = workspace.dual_pane {
                self.dual_pane = dual_pane;
            }
            if let Some(vertical_split) = workspace.vertical_split {
                self.vertical_split = vertical_split;
            }
            if let Some(split_position) = workspace.split_position {
                self.split_position = split_position.max(120);
            }
            if let Some(sidebar_visible) = workspace.sidebar_visible {
                self.sidebar_visible = sidebar_visible;
            }
            if let Some(preview_visible) = workspace.preview_visible {
                self.preview_visible = preview_visible;
            }
            if let Some(preview_width) = workspace.preview_width {
                self.preview_width = preview_width.clamp(PREVIEW_MIN_WIDTH, PREVIEW_MAX_WIDTH);
            }
            self.active_pane = if workspace.active_pane == Some(1) {
                PaneId::Right
            } else {
                PaneId::Left
            };
            self.refresh_grid_column_estimates();
            self.start_listing(PaneId::Left, sender);
            self.start_listing(PaneId::Right, sender);
            self.start_preview(sender);
            self.focus_active_files();
            self.persist_session();
        }
    }

    pub(super) fn on_move_bookmark(&mut self, from: usize, before: usize) {
        if from < self.bookmarks.len() && before < self.bookmarks.len() && from != before {
            let bookmark = self.bookmarks.remove(from);
            let target = if from < before { before - 1 } else { before };
            self.bookmarks.insert(target, bookmark);
            self.persist_session();
        }
    }

    pub(super) fn on_move_bookmark_to_end(&mut self, from: usize) {
        if from < self.bookmarks.len() && from + 1 != self.bookmarks.len() {
            let bookmark = self.bookmarks.remove(from);
            self.bookmarks.push(bookmark);
            self.persist_session();
        }
    }

    pub(super) fn on_create_favorite_group(&mut self, name: String) {
        let name = name.trim();
        if !name.is_empty()
            && !self
                .favorite_groups
                .iter()
                .any(|group| group.name.eq_ignore_ascii_case(name))
        {
            self.favorite_groups.push(FavoriteGroupSession {
                name: name.to_owned(),
                paths: Vec::new(),
            });
            self.persist_session();
        }
    }

    pub(super) fn on_add_current_to_favorite_group(&mut self, index: usize) {
        let path = self.pane(self.active_pane).current_directory().to_string();
        if let Some(group) = self.favorite_groups.get_mut(index)
            && !group.paths.contains(&path)
        {
            group.paths.push(path);
            self.persist_session();
        }
    }

    pub(super) fn on_remove_grouped_favorite(&mut self, group: usize, item: usize) {
        if let Some(group) = self.favorite_groups.get_mut(group)
            && item < group.paths.len()
        {
            group.paths.remove(item);
            self.persist_session();
        }
    }

    pub(super) fn on_connect_remote(&mut self, uri: String, sender: &ComponentSender<Self>) {
        match remote::RemoteConnection::parse(&uri) {
            Ok(connection) => self.on_connect_remote_with_options(connection, None, sender),
            Err(error) => self.pane_mut(self.active_pane).error = Some(error.to_owned()),
        }
    }

    pub(super) fn on_connect_remote_with_options(
        &mut self,
        connection: remote::RemoteConnection,
        replacing: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(old) = replacing {
            self.remote_uris.retain(|uri| *uri != old);
        }
        if !self.remote_uris.contains(&connection.uri) {
            self.remote_uris.push(connection.uri.clone());
        }
        self.persist_session();
        connect_remote_uri(connection, sender);
    }

    pub(super) fn on_remote_connected(
        &mut self,
        uri: String,
        result: Result<VPath, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(path) => self.navigate(self.active_pane, path, sender),
            Err(error) => {
                self.push_operation_log(format!("Could not connect to {uri}: {error}"));
                show_remote_error(uri, error, sender);
            }
        }
    }
}
