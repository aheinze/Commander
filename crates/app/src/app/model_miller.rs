//! Refresh the visible branch as one generation, preserving names rather than row indices.
use super::*;

#[derive(Debug)]
pub(crate) struct MillerSnapshot {
    pub path: VPath,
    pub base: Arc<Listing>,
    pub visible: Arc<Listing>,
}

pub(super) fn load_column(
    vfs: Arc<dyn Vfs>,
    path: VPath,
    sort: SortSpec,
    cancel: &CancelToken,
) -> Result<Arc<Listing>, IndexError> {
    let task = ListingTask::spawn(vfs, ListingRequest { path, sort })?;
    loop {
        if cancel.is_cancelled() {
            task.cancel();
            return Err(IndexError::Cancelled);
        }
        match task.receiver().recv_timeout(Duration::from_millis(50)) {
            Ok(ListingEvent::Complete { listing, .. }) => return Ok(listing),
            Ok(ListingEvent::Failed(error)) => return Err(error),
            Ok(ListingEvent::Cancelled) => return Err(IndexError::Cancelled),
            Ok(ListingEvent::Snapshot { .. }) => {}
            Err(error) if error.is_timeout() => {}
            Err(_) => return Err(IndexError::WorkerPanicked),
        }
    }
}

pub(super) fn visible_column(
    base: &Arc<Listing>,
    query: &str,
    hidden: bool,
    cancel: &CancelToken,
) -> Result<Arc<Listing>, IndexError> {
    if query.is_empty() {
        return Ok(if hidden {
            base.clone()
        } else {
            base.without_hidden()
        });
    }
    let mut filter = FuzzyFilter::try_new(
        base,
        &Filter {
            query: query.to_owned(),
            include_hidden: hidden,
        },
        cancel,
    )?;
    filter.finish_cancellable(cancel)?;
    Ok(base.with_row_order(filter.matched_indices()))
}

impl AppModel {
    pub(super) fn refresh_miller(
        &mut self,
        pane: PaneId,
        reload: bool,
        sender: &ComponentSender<Self>,
    ) {
        self.sync_directory_watches(sender);
        // A filter/visibility change must not replace an in-flight disk refresh
        // with an older cached snapshot.
        let reload = reload || self.pane(pane).loading;
        let state = self.pane_mut(pane);
        state.cancel_work();
        state.reap_workers();
        state.generation = state.generation.wrapping_add(1);
        state.miller_generation = state.miller_generation.wrapping_add(1);
        state.filter_generation = state.filter_generation.wrapping_add(1);
        let generation = state.miller_generation;
        state.loading = reload;
        state.filtering = !reload;
        state.error = None;
        let columns = state
            .miller_columns
            .iter_mut()
            .map(|column| {
                column.loading = true;
                column.error = None;
                (
                    column.path.clone(),
                    if reload {
                        None
                    } else {
                        column.base_listing.clone()
                    },
                )
            })
            .collect::<Vec<_>>();
        state.miller_revision = state.miller_revision.wrapping_add(1);
        let sort = state.sort;
        let hidden = state.show_hidden;
        let query = state.filter_query.clone();
        let cancel = CancelToken::new();
        state.miller_cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        let worker = thread::Builder::new()
            .name("commander-columns".to_owned())
            .spawn(move || {
                let mut snapshots = Vec::new();
                let count = columns.len();
                for (index, (path, cached)) in columns.into_iter().enumerate() {
                    if cancel.is_cancelled() {
                        return;
                    }
                    // An ancestor remains visible while the last column is filtered.
                    let result = cached
                        .map_or_else(|| load_column(vfs.clone(), path.clone(), sort, &cancel), Ok)
                        .and_then(|base| {
                            let visible = visible_column(
                                &base,
                                if index + 1 == count { &query } else { "" },
                                hidden,
                                &cancel,
                            )?;
                            Ok(MillerSnapshot {
                                path: path.clone(),
                                base,
                                visible,
                            })
                        });
                    let failed = result.is_err();
                    snapshots.push((path, result));
                    if failed {
                        break;
                    }
                }
                let _ = input.send(AppMsg::MillerRefreshed {
                    pane,
                    generation,
                    snapshots,
                });
            });
        match worker {
            Ok(worker) => self.pane_mut(pane).workers.push(worker),
            Err(error) => {
                let state = self.pane_mut(pane);
                state.loading = false;
                state.filtering = false;
                state.miller_cancel = None;
                for column in &mut state.miller_columns {
                    column.loading = false;
                }
                state.error = Some(format!("Could not refresh columns: {error}"));
            }
        }
    }

    pub(super) fn on_miller_refreshed(
        &mut self,
        pane: PaneId,
        generation: u64,
        snapshots: Vec<(VPath, Result<MillerSnapshot, IndexError>)>,
        sender: &ComponentSender<Self>,
    ) {
        let state = self.pane_mut(pane);
        if generation != state.miller_generation || state.view_mode != PaneViewMode::Columns {
            return;
        }
        state.miller_cancel = None;
        state.loading = false;
        state.filtering = false;
        for (index, (path, result)) in snapshots.into_iter().enumerate() {
            if state
                .miller_columns
                .get(index)
                .is_none_or(|column| column.path != path)
            {
                break;
            }
            if index > 0 {
                let parent = &state.miller_columns[index - 1];
                let exists = parent.base_listing.as_ref().is_some_and(|listing| {
                    listing.rows().any(|entry| {
                        entry.kind() == EntryKind::Directory
                            && parent.path.join_name(entry.name()) == path
                    })
                });
                if !exists {
                    state.miller_columns.truncate(index);
                    state.restore_names.clear();
                    break;
                }
            }
            let child = state
                .miller_columns
                .get(index + 1)
                .map(|column| column.path.clone());
            let column = &mut state.miller_columns[index];
            let selected = column
                .selected_row
                .and_then(|row| column.listing.as_ref()?.row(row as usize))
                .map(|entry| entry.name().to_os_string())
                .or_else(|| column.restore_name.clone());
            column.loading = false;
            match result {
                Ok(snapshot) => {
                    debug_assert_eq!(snapshot.path, path);
                    column.selected_row = snapshot
                        .visible
                        .rows()
                        .position(|entry| {
                            child.as_ref().map_or_else(
                                || selected.as_ref().is_some_and(|name| name == entry.name()),
                                |child| column.path.join_name(entry.name()) == *child,
                            )
                        })
                        .map(|row| row as u32)
                        .or_else(|| (!snapshot.visible.is_empty()).then_some(0));
                    column.base_listing = Some(snapshot.base);
                    column.listing = Some(snapshot.visible);
                    column.error = None;
                    column.restore_name = None;
                }
                Err(error) => {
                    column.error = Some(error.to_string());
                    column.base_listing = None;
                    column.listing = None;
                    column.selected_row = None;
                    state.miller_columns.truncate(index + 1);
                    break;
                }
            }
        }
        state.miller_focus = state.miller_columns.last().and_then(|column| {
            let entry = column
                .listing
                .as_ref()?
                .row(column.selected_row? as usize)?;
            Some((column.path.join_name(entry.name()), entry.kind()))
        });
        state.retain_miller_selection();
        if let Some(root) = state.miller_columns.first() {
            let base = root.base_listing.clone();
            let listing = root.listing.clone();
            state.active_mut().base_listing = base;
            state.active_mut().listing = listing;
        }
        state.restore_selection();
        state.reveal_pending_item();
        state.revision = state.revision.wrapping_add(1);
        state.selection_revision = state.selection_revision.wrapping_add(1);
        state.miller_revision = state.miller_revision.wrapping_add(1);
        self.persist_session();
        if pane == self.active_pane {
            self.start_preview(sender);
        }
    }
}
