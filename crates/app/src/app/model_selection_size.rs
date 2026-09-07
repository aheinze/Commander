//! Cancellable, pane-local size totals, independent of viewport metadata and the inspector.

use super::*;

impl PaneState {
    pub(super) fn selection_size_label(&self) -> String {
        let measure = &self.selection_size;
        if measure.loading {
            "Calculating size…".to_owned()
        } else if let Some(result) = &measure.result {
            let size = format_size(result.bytes, EntryKind::File);
            if result.skipped == 0 {
                size
            } else if result.bytes == 0 {
                "Size unavailable".to_owned()
            } else {
                format!("{size} (partial)")
            }
        } else if measure.error.is_some() {
            "Size unavailable".to_owned()
        } else {
            "Calculating size…".to_owned()
        }
    }

    pub(super) fn finish_selection_size(
        &mut self,
        generation: u64,
        result: Result<FolderMeasureResult, String>,
    ) {
        let measure = &mut self.selection_size;
        if generation != measure.generation || !measure.loading {
            return;
        }
        measure.loading = false;
        measure.cancel = None;
        match result {
            Ok(result) => measure.result = Some(result),
            Err(error) => measure.error = Some(error),
        }
    }
}

impl AppModel {
    pub(super) fn sync_selection_size(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        let state = self.pane_mut(pane);
        // Metadata hydration and repaints must not repeatedly walk selected folders.
        // Listing/watch revisions also invalidate sizes when selected files change.
        let stamp = [
            state.generation,
            state.selection_revision,
            state.revision,
            state.miller_generation,
        ];
        if state.selection_size_stamp == Some(stamp) {
            return;
        }
        state.selection_size.reset();
        state.selection_size_stamp = Some(stamp);
        if state.selection.is_empty() {
            return;
        }
        if state.loading || state.filtering {
            state.selection_size_stamp = None;
            return;
        }
        state.selection_size.loading = true;
        let generation = state.selection_size.generation;
        let input = sender.input_sender().clone();
        // Coalesce rapid keyboard/range selection changes before starting disk I/O.
        glib::timeout_add_local_once(Duration::from_millis(120), move || {
            let _ = input.send(AppMsg::MeasureSelection { pane, generation });
        });
    }

    pub(super) fn measure_selection(
        &mut self,
        pane: PaneId,
        generation: u64,
        sender: &ComponentSender<Self>,
    ) {
        let state = self.pane_mut(pane);
        if state.selection_size.generation != generation || !state.selection_size.loading {
            return;
        }
        let selection = state.selection.clone();
        let listings = if state.view_mode == PaneViewMode::Columns {
            state
                .miller_columns
                .iter()
                .filter_map(|column| column.listing.clone())
                .collect()
        } else {
            state.active().listing.iter().cloned().collect()
        };
        let cancel = CancelToken::new();
        state.selection_size.cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-selection-size".to_owned())
            .spawn(move || {
                let result = measure_selected_entries(vfs.as_ref(), listings, &selection, &cancel);
                if !cancel.is_cancelled() {
                    let _ = input.send(AppMsg::SelectionSizeReady {
                        pane,
                        generation,
                        result,
                    });
                }
            }) {
            Ok(worker) => {
                let state = self.pane_mut(pane);
                state.reap_workers();
                state.workers.push(worker);
            }
            Err(error) => self
                .pane_mut(pane)
                .finish_selection_size(generation, Err(error.to_string())),
        }
    }
}

fn measure_selected_entries(
    vfs: &dyn Vfs,
    listings: Vec<Arc<Listing>>,
    selection: &Selection,
    cancel: &CancelToken,
) -> Result<FolderMeasureResult, String> {
    cancel
        .check()
        .map_err(|_| "Size calculation cancelled".to_owned())?;
    let mut paths = BTreeSet::new();
    let mut found = BTreeSet::new();
    // Resolve selection keys in the worker, including selected Miller ancestors.
    // Stat every selected path so offscreen/lazily hydrated rows count as well.
    for listing in listings {
        for entry in listing.rows() {
            cancel
                .check()
                .map_err(|_| "Size calculation cancelled".to_owned())?;
            let key = SelectionKey::for_entry(listing.parent(), entry);
            if selection.contains(&key) {
                found.insert(key);
                paths.insert(listing.parent().join_name(entry.name()));
            }
        }
    }
    let mut result = measure_folder_paths(vfs, &paths.into_iter().collect::<Vec<_>>(), cancel)?;
    result.skipped = result
        .skipped
        .saturating_add(selection.len().saturating_sub(found.len()) as u64);
    Ok(result)
}

#[cfg(test)]
#[path = "selection_size_tests.rs"]
mod tests;
