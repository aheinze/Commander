//! Native directory notifications, bounded batching, and background snapshot updates.

use super::*;
use dualpane_index::{
    WatchApplyResult, WatchBatch, WatchChange, WatchChangeKind, apply_watch_batch,
};

const SETTLE: Duration = Duration::from_millis(200);
const MAX_CHANGES: usize = 4_096;

#[derive(Default, Debug)]
struct Pending {
    paths: BTreeSet<VPath>,
    rescan: bool,
}

impl Pending {
    fn add(&mut self, path: VPath) {
        if !self.rescan {
            self.paths.insert(path);
            if self.paths.len() > MAX_CHANGES {
                self.rescan = true;
                self.paths.clear();
            }
        }
    }

    fn merge(&mut self, batch: WatchBatch) {
        self.rescan |= batch.rescan_required;
        if self.rescan {
            self.paths.clear();
        } else {
            for change in batch.changes {
                self.add(change.path);
            }
        }
    }

    fn batch(self) -> WatchBatch {
        WatchBatch {
            changes: self
                .paths
                .into_iter()
                .map(|path| WatchChange {
                    path,
                    kind: WatchChangeKind::Changed,
                })
                .collect(),
            rescan_required: self.rescan,
            errors: Vec::new(),
        }
    }
}

struct Watch {
    id: u64,
    monitor: Option<gio::FileMonitor>,
    timer: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(monitor) = &self.monitor {
            monitor.cancel();
        }
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
    }
}

impl Watch {
    fn new(path: VPath, id: u64, sender: &ComponentSender<AppModel>) -> Self {
        let timer = Rc::new(RefCell::new(None));
        let input = sender.input_sender().clone();
        let monitor = gio::File::for_path(path.as_path())
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE);
        match monitor {
            Ok(monitor) => {
                let pending = Rc::new(RefCell::new(Pending::default()));
                let source = timer.clone();
                monitor.connect_changed(move |_, file, other, event| {
                    let mut changes = pending.borrow_mut();
                    for file in std::iter::once(file).chain(other) {
                        if let Some(changed) = file.path() {
                            if changed == path.as_path() {
                                changes.rescan = true;
                            } else if changed.parent() == Some(path.as_path()) {
                                changes.add(VPath::from(changed));
                            }
                        }
                    }
                    if matches!(
                        event,
                        gio::FileMonitorEvent::Unmounted | gio::FileMonitorEvent::PreUnmount
                    ) {
                        changes.rescan = true;
                    }
                    if source.borrow().is_some() || (changes.paths.is_empty() && !changes.rescan) {
                        return;
                    }
                    let pending = pending.clone();
                    let source_for_callback = source.clone();
                    let path = path.clone();
                    let input = input.clone();
                    *source.borrow_mut() = Some(glib::timeout_add_local_once(SETTLE, move || {
                        source_for_callback.borrow_mut().take();
                        let batch = std::mem::take(&mut *pending.borrow_mut()).batch();
                        let _ = input.send(AppMsg::FilesystemChanged {
                            path,
                            watch_id: id,
                            batch,
                        });
                    }));
                });
                Self {
                    id,
                    monitor: Some(monitor),
                    timer,
                }
            }
            Err(error) => {
                tracing::warn!(%path, %error, "native directory monitoring unavailable; checking every five seconds");
                // No busy-loop on unsupported mounts or unavailable directories.
                *timer.borrow_mut() =
                    Some(glib::timeout_add_local(Duration::from_secs(5), move || {
                        let _ = input.send(AppMsg::FilesystemChanged {
                            path: path.clone(),
                            watch_id: id,
                            batch: Pending {
                                rescan: true,
                                ..Pending::default()
                            }
                            .batch(),
                        });
                        glib::ControlFlow::Continue
                    }));
                Self {
                    id,
                    monitor: None,
                    timer,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp(u64, u64, u64);

fn stamp(state: &PaneState) -> Stamp {
    Stamp(
        state.generation,
        state.miller_generation,
        state.filter_generation,
    )
}

#[derive(Default)]
pub(super) struct LiveUpdates {
    watches: BTreeMap<VPath, Watch>,
    targets: [BTreeSet<VPath>; 2],
    pending: [BTreeMap<VPath, Pending>; 2],
    inflight: [Option<(u64, Stamp, CancelToken)>; 2],
    serial: u64,
    preview_dirty: Option<VPath>,
}

impl LiveUpdates {
    pub(super) fn stop(&mut self) {
        self.watches.clear();
        for (_, _, cancel) in self.inflight.iter().flatten() {
            cancel.cancel();
        }
    }
}

#[derive(Debug)]
struct UpdatedDirectory {
    base: Arc<Listing>,
    visible: Arc<Listing>,
    rows: HashMap<SelectionKey, u32>,
}

#[derive(Debug)]
pub(crate) struct LiveResult {
    pane: PaneId,
    id: u64,
    stamp: Stamp,
    batches: BTreeMap<VPath, WatchBatch>,
    directories: Vec<(VPath, Result<UpdatedDirectory, IndexError>)>,
}

fn directory_snapshot(
    state: &PaneState,
    path: &VPath,
) -> (Option<Arc<Listing>>, Option<Arc<Listing>>) {
    if state.view_mode == PaneViewMode::Columns {
        if let Some(column) = state
            .miller_columns
            .iter()
            .find(|column| &column.path == path)
        {
            return (column.base_listing.clone(), column.listing.clone());
        }
    } else if &state.active().path == path {
        return (
            state.active().base_listing.clone(),
            state.active().listing.clone(),
        );
    }
    (None, None)
}

impl AppModel {
    pub(super) fn sync_directory_watches(&mut self, sender: &ComponentSender<Self>) {
        let mut targets: [BTreeSet<VPath>; 2] = Default::default();
        for pane in [PaneId::Left, PaneId::Right] {
            let state = self.pane(pane);
            if self.dual_pane || self.active_pane == pane {
                if state.view_mode == PaneViewMode::Columns && !state.miller_columns.is_empty() {
                    targets[pane.index()].extend(
                        state
                            .miller_columns
                            .iter()
                            .map(|column| column.path.clone()),
                    );
                } else {
                    targets[pane.index()].insert(state.active().path.clone());
                }
            }
            if let Some((_, previous, cancel)) = &self.live_updates.inflight[pane.index()]
                && (*previous != stamp(state)
                    || targets[pane.index()] != self.live_updates.targets[pane.index()])
            {
                cancel.cancel();
            }
            // Reopening a cached/hidden view must cover changes made while unwatched.
            for path in targets[pane.index()].difference(&self.live_updates.targets[pane.index()]) {
                if !self.pane(pane).loading && directory_snapshot(self.pane(pane), path).0.is_some()
                {
                    self.live_updates.pending[pane.index()].insert(
                        path.clone(),
                        Pending {
                            rescan: true,
                            ..Pending::default()
                        },
                    );
                }
            }
            self.live_updates.pending[pane.index()]
                .retain(|path, _| targets[pane.index()].contains(path));
        }
        let all = targets
            .iter()
            .flat_map(|paths| paths.iter().cloned())
            .collect::<BTreeSet<_>>();
        self.live_updates
            .watches
            .retain(|path, _| all.contains(path));
        for path in all {
            if !self.live_updates.watches.contains_key(&path) {
                self.live_updates.serial = self.live_updates.serial.wrapping_add(1);
                self.live_updates.watches.insert(
                    path.clone(),
                    Watch::new(path, self.live_updates.serial, sender),
                );
            }
        }
        self.live_updates.targets = targets;
    }

    pub(super) fn on_filesystem_changed(
        &mut self,
        path: VPath,
        watch_id: u64,
        batch: WatchBatch,
        sender: &ComponentSender<Self>,
    ) {
        if self
            .live_updates
            .watches
            .get(&path)
            .is_none_or(|watch| watch.id != watch_id)
        {
            return;
        }
        if batch.rescan_required {
            // Reattach after a watched directory is replaced, removed, or remounted.
            self.live_updates.watches.remove(&path);
            self.live_updates
                .watches
                .insert(path.clone(), Watch::new(path.clone(), watch_id, sender));
        }
        if let Some(preview) = &self.preview_state.path
            && (preview == &path
                || (batch.rescan_required && preview.parent().as_ref() == Some(&path))
                || batch.changes.iter().any(|change| &change.path == preview))
        {
            self.live_updates.preview_dirty = Some(preview.clone());
            self.preview_state.cancel();
            self.preview_state.generation = self.preview_state.generation.wrapping_add(1);
        }
        for pane in [PaneId::Left, PaneId::Right] {
            if self.live_updates.targets[pane.index()].contains(&path) {
                self.live_updates.pending[pane.index()]
                    .entry(path.clone())
                    .or_default()
                    .merge(batch.clone());
            }
        }
    }

    pub(super) fn flush_filesystem_changes(&mut self, sender: &ComponentSender<Self>) {
        for pane in [PaneId::Left, PaneId::Right] {
            let index = pane.index();
            let state = self.pane(pane);
            if self.live_updates.inflight[index].is_some()
                || state.loading
                || state.filtering
                || self.live_updates.pending[index].is_empty()
            {
                continue;
            }
            let version = stamp(state);
            let sort = state.sort;
            let hidden = state.show_hidden;
            let query = state.filter_query.clone();
            let current = state.current_directory().clone();
            let batches = std::mem::take(&mut self.live_updates.pending[index])
                .into_iter()
                .map(|(path, pending)| (path, pending.batch()))
                .collect::<BTreeMap<_, _>>();
            let snapshots = batches
                .keys()
                .map(|path| (path.clone(), directory_snapshot(self.pane(pane), path)))
                .collect::<Vec<_>>();
            let vfs = self.vfs.clone();
            let cancel = CancelToken::new();
            let worker_cancel = cancel.clone();
            self.live_updates.serial = self.live_updates.serial.wrapping_add(1);
            let id = self.live_updates.serial;
            let input = sender.input_sender().clone();
            let retry = batches.clone();
            let worker = thread::Builder::new()
                .name("commander-live-update".to_owned())
                .spawn(move || {
                    let directories = snapshots
                        .into_iter()
                        .map(|(path, (base, visible))| {
                            let batch = &batches[&path];
                            let result = (|| {
                                let old = base.as_ref().map(|base| {
                                    visible.as_ref().map_or_else(
                                        || base.clone(),
                                        |view| base.with_cached_metadata_from(view),
                                    )
                                });
                                let updated = match old.as_ref().filter(|_| !batch.rescan_required)
                                {
                                    Some(base) => match apply_watch_batch(
                                        vfs.as_ref(),
                                        base,
                                        batch,
                                        &worker_cancel,
                                    )? {
                                        WatchApplyResult::Updated(updated) => updated,
                                        WatchApplyResult::RescanRequired => {
                                            model_miller::load_column(
                                                vfs.clone(),
                                                path.clone(),
                                                sort,
                                                &worker_cancel,
                                            )?
                                        }
                                    },
                                    None => model_miller::load_column(
                                        vfs.clone(),
                                        path.clone(),
                                        sort,
                                        &worker_cancel,
                                    )?,
                                };
                                let shared = old
                                    .as_ref()
                                    .is_some_and(|old| updated.shares_rows_with(old));
                                let filtered = if shared {
                                    visible
                                        .as_ref()
                                        .and_then(|view| updated.with_view_order_from(view))
                                } else {
                                    None
                                };
                                let filtered = match filtered {
                                    Some(filtered) => filtered,
                                    None => model_miller::visible_column(
                                        &updated,
                                        if path == current { &query } else { "" },
                                        hidden,
                                        &worker_cancel,
                                    )?,
                                };
                                let mut rows = HashMap::new();
                                if !visible
                                    .as_ref()
                                    .is_some_and(|old| filtered.shares_rows_with(old))
                                {
                                    for (row, entry) in filtered.rows().enumerate() {
                                        worker_cancel.check().map_err(|_| IndexError::Cancelled)?;
                                        rows.entry(SelectionKey::for_entry(
                                            filtered.parent(),
                                            entry,
                                        ))
                                        .or_insert(row as u32);
                                    }
                                }
                                Ok(UpdatedDirectory {
                                    base: updated,
                                    visible: filtered,
                                    rows,
                                })
                            })();
                            (path, result)
                        })
                        .collect();
                    let _ = input.send(AppMsg::FilesystemUpdated(LiveResult {
                        pane,
                        id,
                        stamp: version,
                        batches,
                        directories,
                    }));
                });
            match worker {
                Ok(worker) => {
                    self.live_updates.inflight[index] = Some((id, version, cancel));
                    self.pane_mut(pane).reap_workers();
                    self.pane_mut(pane).workers.push(worker);
                }
                Err(error) => {
                    for (path, batch) in retry {
                        self.live_updates.pending[index]
                            .entry(path)
                            .or_default()
                            .merge(batch);
                    }
                    self.pane_mut(pane).error = Some(format!("Could not update folder: {error}"));
                }
            }
        }
    }

    pub(super) fn on_filesystem_updated(
        &mut self,
        result: LiveResult,
        sender: &ComponentSender<Self>,
    ) {
        let pane = result.pane;
        let index = pane.index();
        if self.live_updates.inflight[index]
            .as_ref()
            .is_none_or(|(id, _, _)| *id != result.id)
        {
            return;
        }
        let (_, _, cancel) = self.live_updates.inflight[index].take().unwrap();
        if cancel.is_cancelled() || stamp(self.pane(pane)) != result.stamp {
            // Rebase on the current view after navigation, filtering, or a manual refresh.
            for (path, batch) in result.batches {
                if self.live_updates.targets[index].contains(&path) {
                    self.live_updates.pending[index]
                        .entry(path)
                        .or_default()
                        .merge(batch);
                }
            }
            return;
        }
        let state = self.pane_mut(pane);
        state.cancel_work();
        state.thumbnail_cancel = CancelToken::new();
        state.generation = state.generation.wrapping_add(1);
        state.filter_generation = state.filter_generation.wrapping_add(1);
        state.miller_generation = state.miller_generation.wrapping_add(1);
        let mut updated_paths = HashSet::new();
        for (path, update) in result.directories {
            updated_paths.insert(path.clone());
            if state.view_mode == PaneViewMode::Columns {
                if let Some(column) = state
                    .miller_columns
                    .iter_mut()
                    .find(|column| column.path == path)
                {
                    match update {
                        Ok(update) => {
                            column.selected_row = retained_cursor(
                                column.listing.as_deref(),
                                column.selected_row,
                                &update,
                            );
                            column.error = None;
                            column.base_listing = Some(update.base);
                            column.listing = Some(update.visible);
                        }
                        Err(error) => {
                            column.error = Some(error.to_string());
                            column.base_listing = None;
                            column.listing = None;
                            column.selected_row = None;
                        }
                    }
                }
            } else if state.active().path == path {
                match update {
                    Ok(update) => {
                        state.cursor_row = retained_cursor(
                            state.active().listing.as_deref(),
                            Some(state.cursor_row),
                            &update,
                        )
                        .unwrap_or(0);
                        let unchanged = state
                            .active()
                            .listing
                            .as_deref()
                            .is_some_and(|old| update.visible.shares_rows_with(old));
                        retain_selection(&mut state.selection, &update.rows, unchanged);
                        state.active_mut().base_listing = Some(update.base);
                        state.active_mut().listing = Some(update.visible);
                        state.error = None;
                    }
                    Err(error) => {
                        state.error = Some(error.to_string());
                        state.active_mut().base_listing = None;
                        state.active_mut().listing = None;
                        state.selection.clear();
                        state.cursor_row = 0;
                    }
                }
            }
        }
        if state.view_mode == PaneViewMode::Columns {
            for index in 1..state.miller_columns.len() {
                let parent = &state.miller_columns[index - 1];
                let child = &state.miller_columns[index];
                if updated_paths.contains(&parent.path)
                    && !parent.base_listing.as_ref().is_some_and(|listing| {
                        listing.rows().any(|entry| {
                            entry.kind() == EntryKind::Directory
                                && child.path.file_name() == Some(entry.name())
                        })
                    })
                {
                    state.miller_columns.truncate(index);
                    break;
                }
            }
            state.retain_miller_selection();
            state.miller_focus = state.miller_columns.last().and_then(|column| {
                let entry = column
                    .listing
                    .as_ref()?
                    .row(column.selected_row? as usize)?;
                Some((column.path.join_name(entry.name()), entry.kind()))
            });
            if let Some(root) = state.miller_columns.first() {
                let base = root.base_listing.clone();
                let visible = root.listing.clone();
                state.active_mut().base_listing = base;
                state.active_mut().listing = visible;
            }
            state.miller_revision = state.miller_revision.wrapping_add(1);
        }
        state.range_anchor = None;
        state.revision = state.revision.wrapping_add(1);
        state.selection_revision = state.selection_revision.wrapping_add(1);
        state.scroll_restore_epoch = state.scroll_restore_epoch.wrapping_add(1);
        if pane == self.active_pane {
            if let Some(dirty) = self.live_updates.preview_dirty.take()
                && Some(&dirty) == self.preview_state.path.as_ref()
            {
                self.preview_state.cancel();
                self.preview_state.path = None;
                self.preview_state.content = None;
                self.preview_state.error = None;
            }
            self.start_preview(sender);
        }
    }
}

fn retained_cursor(
    old: Option<&Listing>,
    row: Option<u32>,
    update: &UpdatedDirectory,
) -> Option<u32> {
    if update.visible.is_empty() {
        return None;
    }
    if old.is_some_and(|old| update.visible.shares_rows_with(old)) {
        return row;
    }
    row.and_then(|row| old?.row(row as usize))
        .and_then(|entry| {
            update
                .rows
                .get(&SelectionKey::for_entry(update.visible.parent(), entry))
                .copied()
        })
        .or_else(|| {
            Some(
                row.unwrap_or(0)
                    .min(update.visible.len().saturating_sub(1) as u32),
            )
        })
}

fn retain_selection(selection: &mut Selection, rows: &HashMap<SelectionKey, u32>, unchanged: bool) {
    if !unchanged && !selection.is_empty() {
        let retained = selection
            .iter()
            .filter(|key| rows.contains_key(*key))
            .cloned()
            .collect();
        selection.retain_available(&retained);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relm4::{Component, ComponentController};

    #[test]
    fn event_bursts_are_deduplicated_and_overflow_requests_one_rescan() {
        let mut pending = Pending::default();
        for _ in 0..10_000 {
            pending.add(VPath::from("/tmp/same"));
        }
        assert_eq!(pending.paths.len(), 1);
        for index in 0..MAX_CHANGES + 100 {
            pending.add(VPath::from(format!("/tmp/file-{index}").as_str()));
        }
        assert!(pending.rescan);
        assert!(pending.paths.is_empty());
        pending.merge(
            Pending {
                paths: BTreeSet::from([VPath::from("/tmp/new")]),
                rescan: false,
            }
            .batch(),
        );
        assert!(pending.batch().rescan_required);
    }

    #[test]
    fn generation_changes_restart_only_pending_thumbnails_in_that_pane() {
        let path = VPath::from("/tmp/image.png");
        let mut thumbnails = ThumbnailUiState::new();
        for pane in 0..2 {
            thumbnails
                .waiting
                .entry(path.clone())
                .or_default()
                .push(ThumbnailTarget {
                    widget_id: pane,
                    key: GridThumbnailKey {
                        pane,
                        generation: 1,
                        path: path.clone(),
                    },
                    stack: glib::WeakRef::new(),
                    picture: glib::WeakRef::new(),
                });
        }
        assert_eq!(
            thumbnails.begin_generation(PaneId::Left, 2),
            HashSet::from([path.clone()])
        );
        assert_eq!(thumbnails.waiting[&path].len(), 1);
        assert_eq!(thumbnails.waiting[&path][0].key.pane, 1);
        assert!(thumbnails.begin_generation(PaneId::Left, 2).is_empty());
    }

    fn thumbnail_rebinding_touches_only_affected_materialized_rows() {
        let fixture = tempfile::tempdir().unwrap();
        for name in ["a.png", "b.png", "c.png"] {
            std::fs::write(fixture.path().join(name), "fixture").unwrap();
        }
        let parent = VPath::from(fixture.path());
        let listing = ListingTask::spawn(
            Arc::new(LocalFs),
            ListingRequest {
                path: parent.clone(),
                sort: SortSpec::default(),
            },
        )
        .unwrap()
        .wait_complete()
        .unwrap()
        .0;
        let model = ListingListModel::new();
        model.set_listing(Some(listing));
        let first = model.item(0).unwrap();
        let second = model.item(1).unwrap();
        let notifications = Rc::new(RefCell::new(Vec::new()));
        let received = notifications.clone();
        model.connect_items_changed(move |_, position, removed, added| {
            received.borrow_mut().push((position, removed, added));
        });
        model.refresh_paths(&HashSet::from([
            parent.join_name("a.png".as_ref()),
            parent.join_name("c.png".as_ref()),
        ]));
        assert_eq!(*notifications.borrow(), [(0, 1, 1)]);
        assert_ne!(first, model.item(0).unwrap());
        assert_eq!(second, model.item(1).unwrap());
        assert_eq!(model.n_items(), 3);
    }

    #[track_caller]
    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(12);
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "timed out at {}",
                std::panic::Location::caller()
            );
            let context = glib::MainContext::default();
            while context.pending() {
                context.iteration(false);
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn has_file(app: &relm4::Controller<AppModel>, pane: PaneId, name: &str) -> bool {
        app.model()
            .selection_listing(pane)
            .is_some_and(|listing| listing.rows().any(|entry| entry.name() == name))
    }

    fn text_is(app: &relm4::Controller<AppModel>, expected: &str) -> bool {
        app.model().preview_state.content.as_ref().is_some_and(|preview| {
            matches!(&preview.payload, PreviewPayload::Text { content, .. } if content == expected)
        })
    }

    #[test]
    #[ignore = "requires GTK and isolated XDG state; run alone with --ignored --test-threads=1"]
    fn gtk_live_updates_cover_views_previews_and_watch_lifetimes() {
        assert!(
            std::env::var_os("COMMANDER_ISOLATED_TEST").is_some(),
            "run with isolated XDG directories"
        );
        adw::init().unwrap();
        thumbnail_rebinding_touches_only_affected_materialized_rows();
        relm4::main_adw_application()
            .register(gio::Cancellable::NONE)
            .unwrap();
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("root");
        let elsewhere = fixture.path().join("elsewhere");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(root.join("anchor.txt"), "before").unwrap();
        std::fs::write(root.join("other.txt"), "other").unwrap();
        std::fs::write(elsewhere.join("elsewhere.txt"), "elsewhere").unwrap();
        let (session_worker, startup) = SessionWorker::start().unwrap();
        let mut session = SessionState {
            dual_pane: true,
            sidebar_visible: false,
            preview_visible: false,
            ..SessionState::default()
        };
        session.left.view_mode = PaneViewMode::List;
        session.right.view_mode = PaneViewMode::Grid;
        let app = AppModel::builder()
            .launch(AppInit {
                options: AppOptions {
                    left: Some(VPath::from(root.as_path())),
                    right: Some(VPath::from(root.as_path())),
                    ..AppOptions::default()
                },
                session_worker,
                session: Some(session),
                keymap_overrides: startup.keymap_overrides,
                history: startup.history,
                history_warning: startup.history_warning,
                started: Instant::now(),
            })
            .detach();
        // Build real GTK models without showing a window or taking desktop focus.
        wait_until(|| {
            has_file(&app, PaneId::Left, "anchor.txt")
                && has_file(&app, PaneId::Right, "anchor.txt")
                && !app.model().pane(PaneId::Left).loading
                && !app.model().pane(PaneId::Right).loading
        });
        assert_eq!(
            app.model().live_updates.watches.len(),
            1,
            "share native watches between panes"
        );
        app.emit(AppMsg::ContextTarget(
            PaneId::Left,
            Some((VPath::from(root.join("anchor.txt")), EntryKind::File)),
        ));
        app.emit(AppMsg::NavigationScroll {
            pane: PaneId::Left,
            path: VPath::from(root.as_path()),
            x: false,
            value: 450,
        });
        wait_until(|| text_is(&app, "before"));
        std::fs::write(root.join("aaa.txt"), "new").unwrap();
        wait_until(|| {
            has_file(&app, PaneId::Left, "aaa.txt") && has_file(&app, PaneId::Right, "aaa.txt")
        });
        assert_eq!(
            app.model().focused_path(PaneId::Left),
            Some(VPath::from(root.join("anchor.txt")))
        );
        assert_eq!(app.model().pane(PaneId::Left).selection.len(), 1);
        assert_eq!(app.model().pane(PaneId::Left).scroll_y, 450);
        std::fs::write(root.join("anchor.txt"), "externally changed").unwrap();
        wait_until(|| text_is(&app, "externally changed"));
        std::fs::rename(root.join("anchor.txt"), root.join("renamed.txt")).unwrap();
        wait_until(|| {
            has_file(&app, PaneId::Left, "renamed.txt")
                && !has_file(&app, PaneId::Right, "anchor.txt")
        });
        assert_eq!(
            app.model().focused_path(PaneId::Left),
            Some(VPath::from(root.join("renamed.txt")))
        );
        assert_eq!(app.model().pane(PaneId::Left).selection.len(), 1);
        std::fs::remove_file(root.join("other.txt")).unwrap();
        wait_until(|| {
            !has_file(&app, PaneId::Left, "other.txt")
                && !has_file(&app, PaneId::Right, "other.txt")
        });

        app.emit(AppMsg::SetPaneFilter(PaneId::Left, "match".to_owned()));
        for index in 0..120 {
            std::fs::write(root.join(format!("match-{index:03}.txt")), "burst").unwrap();
        }
        std::fs::write(root.join("unrelated.txt"), "unrelated").unwrap();
        std::fs::write(root.join(".match-hidden"), "hidden").unwrap();
        wait_until(|| {
            has_file(&app, PaneId::Left, "match-119.txt")
                && !app.model().pane(PaneId::Left).filtering
        });
        assert!(!has_file(&app, PaneId::Left, "unrelated.txt"));
        assert!(!has_file(&app, PaneId::Left, ".match-hidden"));
        assert_eq!(
            app.model().selection_listing(PaneId::Left).unwrap().len(),
            120
        );

        app.emit(AppMsg::SetPaneFilter(PaneId::Left, String::new()));
        let child = root.join("Child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("note.txt"), "child before").unwrap();
        wait_until(|| {
            has_file(&app, PaneId::Left, "Child") && !app.model().pane(PaneId::Left).filtering
        });
        app.emit(AppMsg::SetViewMode(PaneViewMode::Columns));
        wait_until(|| !app.model().pane(PaneId::Left).miller_columns.is_empty());
        let row = app.model().pane(PaneId::Left).miller_columns[0]
            .listing
            .as_ref()
            .unwrap()
            .rows()
            .position(|entry| entry.name() == "Child")
            .unwrap() as u32;
        app.emit(AppMsg::MillerOpen(PaneId::Left, 0, row));
        wait_until(|| has_file(&app, PaneId::Left, "note.txt"));
        assert_eq!(
            app.model().focused_path(PaneId::Left),
            Some(VPath::from(child.as_path()))
        );
        app.emit(AppMsg::ContextTarget(
            PaneId::Left,
            Some((VPath::from(child.join("note.txt")), EntryKind::File)),
        ));
        wait_until(|| text_is(&app, "child before"));
        assert_eq!(app.model().live_updates.watches.len(), 2);
        std::fs::write(child.join("note.txt"), "child after").unwrap();
        std::fs::write(root.join("sibling.txt"), "sibling").unwrap();
        wait_until(|| {
            text_is(&app, "child after")
                && app.model().pane(PaneId::Left).miller_columns[0]
                    .listing
                    .as_ref()
                    .unwrap()
                    .rows()
                    .any(|entry| entry.name() == "sibling.txt")
        });
        assert_eq!(app.model().pane(PaneId::Left).miller_columns.len(), 2);
        let old_watch = app.model().live_updates.watches[&VPath::from(child.as_path())].id;
        std::fs::remove_dir_all(&child).unwrap();
        wait_until(|| {
            app.model().pane(PaneId::Left).miller_columns.len() == 1
                && app.model().live_updates.watches.len() == 1
        });
        app.emit(AppMsg::FilesystemChanged {
            path: VPath::from(child.as_path()),
            watch_id: old_watch,
            batch: Pending {
                rescan: true,
                ..Pending::default()
            }
            .batch(),
        });

        app.emit(AppMsg::Navigate(
            PaneId::Left,
            VPath::from(elsewhere.as_path()),
        ));
        wait_until(|| {
            has_file(&app, PaneId::Left, "elsewhere.txt") && !app.model().pane(PaneId::Left).loading
        });
        app.emit(AppMsg::ToggleDualPane);
        wait_until(|| !app.model().dual_pane && app.model().live_updates.watches.len() == 1);
        std::fs::write(root.join("while-hidden.txt"), "hidden pane change").unwrap();
        app.emit(AppMsg::ToggleDualPane);
        wait_until(|| has_file(&app, PaneId::Right, "while-hidden.txt"));
        assert!(!has_file(&app, PaneId::Left, "while-hidden.txt"));
        assert_eq!(app.model().live_updates.watches.len(), 2);
        app.widget().close();
    }
}
