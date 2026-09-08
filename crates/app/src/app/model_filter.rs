//! Fuzzy filtering, glob selection, and the pane-local filter workers.

use super::*;

impl AppModel {
    pub(super) fn set_filter(
        &mut self,
        pane: PaneId,
        query: String,
        sender: &ComponentSender<Self>,
    ) {
        if self.pane(pane).filter_query == query {
            return;
        }
        let state = self.pane_mut(pane);
        state.filter_query = query;
        state.filter_elapsed = None;
        state.cursor_row = 0;
        state.range_anchor = None;
        self.start_filter(pane, sender);
    }

    pub(super) fn start_filter(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        if self.pane(pane).view_mode == PaneViewMode::Columns
            && !self.pane(pane).miller_columns.is_empty()
        {
            self.refresh_miller(pane, false, sender);
            return;
        }
        if self.pane(pane).filter_query.is_empty() {
            let state = self.pane_mut(pane);
            if let Some(cancel) = state.metadata_cancel.take() {
                cancel.cancel();
            }
            state.metadata_inflight = false;
            state.metadata_pending.clear();
            state.filter_generation = state.filter_generation.wrapping_add(1);
            let Some(base_listing) = state.active().base_listing.clone() else {
                state.filtering = false;
                return;
            };
            state.active_mut().listing = Some(if state.show_hidden {
                base_listing
            } else {
                base_listing.without_hidden()
            });
            state.filtering = false;
            state.revision = state.revision.wrapping_add(1);
            let _ = state;
            self.sync_miller_root(pane);
            return;
        }
        if self.pane(pane).filter_sender.is_none() {
            self.ensure_filter_worker(pane, sender);
        }
        let (query, generation, filter_sender) = {
            let state = self.pane_mut(pane);
            if let Some(cancel) = state.metadata_cancel.take() {
                cancel.cancel();
            }
            state.metadata_inflight = false;
            state.metadata_pending.clear();
            state.filter_generation = state.filter_generation.wrapping_add(1);
            let generation = state.filter_generation;
            if state.active().base_listing.is_none() {
                state.filtering = false;
                return;
            }
            state.filtering = true;
            (
                state.filter_query.clone(),
                generation,
                state.filter_sender.clone(),
            )
        };
        let Some(filter_sender) = filter_sender else {
            self.pane_mut(pane).filtering = false;
            return;
        };
        if filter_sender
            .send(PaneFilterCommand::Query {
                generation,
                query,
                queued_at: Instant::now(),
            })
            .is_err()
        {
            let state = self.pane_mut(pane);
            state.filtering = false;
            state.filter_sender = None;
            state.error = Some("filter worker stopped unexpectedly".to_owned());
        }
    }

    pub(super) fn ensure_filter_worker(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        if self.pane(pane).view_mode == PaneViewMode::Columns
            || self.pane(pane).filter_sender.is_some()
        {
            return;
        }
        let Some(listing) = self.pane(pane).active().base_listing.clone() else {
            return;
        };
        if !listing.is_complete() {
            return;
        }
        let include_hidden = self.pane(pane).show_hidden;
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        let (command_sender, command_receiver) = std::sync::mpsc::channel();
        let input = sender.input_sender().clone();
        let worker = thread::Builder::new()
            .name(format!("dualpane-{}-filter", pane.label().to_lowercase()))
            .spawn(move || {
                let mut engine = match FuzzyFilter::try_new(
                    &listing,
                    &Filter {
                        query: String::new(),
                        include_hidden,
                    },
                    &worker_cancel,
                ) {
                    Ok(engine) => engine,
                    Err(_) => return,
                };
                if engine.finish_cancellable(&worker_cancel).is_err() {
                    return;
                }
                if input.send(AppMsg::FilterWorkerReady(pane)).is_err() {
                    return;
                }
                while let Ok(mut command) = command_receiver.recv() {
                    loop {
                        for newer in command_receiver.try_iter() {
                            command = newer;
                        }
                        let PaneFilterCommand::Query {
                            generation,
                            query,
                            queued_at,
                        } = command;
                        let queue_delay = queued_at.elapsed();
                        let started = Instant::now();
                        engine.update_query(&query);
                        if engine.finish_cancellable(&worker_cancel).is_err() {
                            return;
                        }
                        if let Some(newer) = command_receiver.try_iter().last() {
                            command = newer;
                            continue;
                        }
                        let row_order = engine.matched_indices();
                        let result = FilterResult {
                            listing: listing.with_row_order(row_order),
                            elapsed: started.elapsed(),
                        };
                        let _ = input.send(AppMsg::FilterReady {
                            pane,
                            generation,
                            queue_delay,
                            result: Ok(result),
                        });
                        break;
                    }
                }
            });
        match worker {
            Ok(worker) => {
                let state = self.pane_mut(pane);
                state.filter_cancel = Some(cancel);
                state.filter_sender = Some(command_sender);
                state.workers.push(worker);
            }
            Err(error) => {
                self.pane_mut(pane).error = Some(format!("failed to start filter: {error}"));
            }
        }
    }

    pub(super) fn handle_filter(
        &mut self,
        pane: PaneId,
        generation: u64,
        queue_delay: Duration,
        result: Result<FilterResult, IndexError>,
    ) {
        let is_benchmark = self.filter_benchmark_started
            && self
                .benchmark_filter
                .as_deref()
                .is_some_and(|query| query == self.pane(pane).filter_query);
        let mut completed_elapsed = None;
        let state = self.pane_mut(pane);
        if generation != state.filter_generation {
            return;
        }
        state.filtering = false;
        match result {
            Ok(result) => {
                tracing::info!(
                    pane = pane.label(),
                    query = %state.filter_query,
                    matches = result.listing.len(),
                    elapsed_ms = result.elapsed.as_secs_f64() * 1_000.0,
                    queue_ms = queue_delay.as_secs_f64() * 1_000.0,
                    "type-ahead results ready"
                );
                state.filter_elapsed = Some(result.elapsed);
                completed_elapsed = Some(result.elapsed);
                state.cursor_row = state
                    .cursor_row
                    .min(u32::try_from(result.listing.len().saturating_sub(1)).unwrap_or(u32::MAX));
                state.active_mut().listing = Some(result.listing);
                state.revision = state.revision.wrapping_add(1);
            }
            Err(IndexError::Cancelled) => {}
            Err(error) => state.error = Some(error.to_string()),
        }
        let _ = state;
        self.sync_miller_root(pane);
        if is_benchmark && completed_elapsed.is_some() {
            let index = pane.index();
            self.filter_benchmark_worker_results[index] = completed_elapsed;
            self.filter_benchmark_results[index] = self.filter_benchmark_started_at[index]
                .map(|started| started.elapsed())
                .or(completed_elapsed);
            let response = self.filter_benchmark_results[index]
                .unwrap_or_default()
                .as_secs_f64()
                * 1_000.0;
            let worker = self.filter_benchmark_worker_results[index]
                .unwrap_or_default()
                .as_secs_f64()
                * 1_000.0;
            println!(
                "DUALPANE_FILTER active_response_ms={response:.3} worker_ms={worker:.3} \
                 queue_ms={:.3} budget_ms=30.000",
                queue_delay.as_secs_f64() * 1_000.0
            );
            relm4::main_application().quit();
        }
    }

    pub(super) fn clear_layered(&mut self, sender: &ComponentSender<Self>) {
        let pane = self.active_pane;
        if self.palette_open {
            self.palette_open = false;
            self.palette_query.clear();
            self.palette_selection = 0;
            self.focus_active_files();
        } else if self.quick_look_open {
            self.quick_look_open = false;
        } else if self.pane(pane).archive_browse.source.is_some() {
            self.cancel_archive_open(pane, sender);
        } else if !self.pane(pane).filter_query.is_empty() {
            self.set_filter(pane, String::new(), sender);
        } else if !self.pane(pane).selection.is_empty() {
            self.clear_selection(pane);
        } else if self.pane(pane).glob_open {
            let state = self.pane_mut(pane);
            state.glob_open = false;
            state.glob_query.clear();
        }
    }

    pub(super) fn start_glob(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        let (listing, pattern, select, generation) = {
            let state = self.pane_mut(pane);
            if let Some(cancel) = state.glob_cancel.take() {
                cancel.cancel();
            }
            let listing = if state.view_mode == PaneViewMode::Columns {
                state
                    .miller_columns
                    .last()
                    .and_then(|column| column.listing.clone())
            } else {
                state.active().listing.clone()
            };
            let Some(listing) = listing else {
                return;
            };
            if state.glob_query.is_empty() {
                return;
            }
            state.glob_generation = state.glob_generation.wrapping_add(1);
            (
                listing,
                state.glob_query.to_lowercase(),
                state.glob_select,
                state.glob_generation,
            )
        };
        let cancel = CancelToken::new();
        self.pane_mut(pane).glob_cancel = Some(cancel.clone());
        let input = sender.input_sender().clone();
        let worker = thread::Builder::new()
            .name(format!("dualpane-{}-glob", pane.label().to_lowercase()))
            .spawn(move || {
                let started = Instant::now();
                let mut keys = Vec::new();
                for (index, entry) in listing.rows().enumerate() {
                    if index.is_multiple_of(256) && cancel.is_cancelled() {
                        return;
                    }
                    if wildcard_matches(&pattern, &entry.name().to_string_lossy().to_lowercase()) {
                        keys.push(SelectionKey::for_entry(listing.parent(), entry));
                    }
                }
                let _ = input.send(AppMsg::GlobReady {
                    pane,
                    generation,
                    select,
                    keys,
                    elapsed: started.elapsed(),
                });
            });
        match worker {
            Ok(worker) => self.pane_mut(pane).workers.push(worker),
            Err(error) => {
                self.pane_mut(pane).glob_cancel = None;
                self.pane_mut(pane).error = Some(format!("failed to match glob: {error}"));
            }
        }
    }

    pub(super) fn handle_glob(
        &mut self,
        pane: PaneId,
        generation: u64,
        select: bool,
        keys: Vec<SelectionKey>,
        elapsed: Duration,
    ) {
        let state = self.pane_mut(pane);
        if generation != state.glob_generation {
            return;
        }
        tracing::info!(
            pane = pane.label(),
            matches = keys.len(),
            elapsed_ms = elapsed.as_secs_f64() * 1_000.0,
            "glob selection ready"
        );
        for key in keys {
            if select {
                state.selection.select_preserving_anchor(key);
            } else {
                state.selection.deselect(&key);
            }
        }
        state.selection_revision = state.selection_revision.wrapping_add(1);
        state.glob_cancel = None;
        state.glob_open = false;
        state.glob_query.clear();
    }

    pub(super) fn selection_key_at(&self, pane: PaneId, row: u32) -> Option<SelectionKey> {
        let listing = self.selection_listing(pane)?;
        let entry = listing.row(row as usize)?;
        Some(SelectionKey::for_entry(listing.parent(), entry))
    }

    pub(super) fn selection_listing(&self, pane: PaneId) -> Option<&Listing> {
        let state = self.pane(pane);
        if state.view_mode == PaneViewMode::Columns {
            state
                .miller_columns
                .last()
                .and_then(|column| column.listing.as_deref())
        } else {
            state.active().listing.as_deref()
        }
    }

    pub(super) fn on_open_glob(&mut self, select: bool) {
        let pane = self.active_pane;
        let state = self.pane_mut(pane);
        state.glob_select = select;
        state.glob_open = true;
        state.glob_query.clear();
    }

    pub(super) fn on_glob_ready(
        &mut self,
        pane: PaneId,
        generation: u64,
        select: bool,
        keys: Vec<SelectionKey>,
        elapsed: Duration,
    ) {
        self.handle_glob(pane, generation, select, keys, elapsed)
    }
}
