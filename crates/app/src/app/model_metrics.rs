//! Lazy metadata hydration, frame accounting, benchmark reporting, and session persistence.

use super::*;

impl AppModel {
    pub(super) fn request_metadata(
        &mut self,
        pane: PaneId,
        row_index: u32,
        sender: &ComponentSender<Self>,
    ) {
        let state = self.pane_mut(pane);
        let Some(listing) = state.active().listing.as_ref() else {
            return;
        };
        let Some(source_index) = listing.source_index_at_row(row_index as usize) else {
            return;
        };
        if !listing.is_complete() || listing.metadata(source_index).is_some() {
            return;
        }
        state.metadata_pending.insert(row_index);
        if !state.metadata_inflight {
            self.start_metadata(pane, sender);
        }
    }

    pub(super) fn start_metadata(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        let (listing, row_index, generation) = {
            let state = self.pane_mut(pane);
            let Some(listing) = state.active().listing.clone() else {
                return;
            };
            let row_index = loop {
                let Some(row_index) = state.metadata_pending.pop_first() else {
                    return;
                };
                let missing = listing
                    .source_index_at_row(row_index as usize)
                    .is_some_and(|source_index| listing.metadata(source_index).is_none());
                if missing {
                    break row_index;
                }
            };
            state.metadata_inflight = true;
            (listing, row_index, state.generation)
        };
        let cancel = CancelToken::new();
        self.pane_mut(pane).metadata_cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        let start = row_index.saturating_sub(METADATA_LOOKBEHIND);
        let end = row_index.saturating_add(METADATA_LOOKAHEAD);
        let worker = thread::Builder::new()
            .name(format!("dualpane-{}-metadata", pane.label().to_lowercase()))
            .spawn(move || {
                let result = hydrate_metadata_rows(vfs.as_ref(), &listing, &cancel, start..end);
                let _ = input.send(AppMsg::MetadataReady {
                    pane,
                    generation,
                    result,
                });
            });
        match worker {
            Ok(worker) => self.pane_mut(pane).workers.push(worker),
            Err(error) => {
                let state = self.pane_mut(pane);
                state.metadata_inflight = false;
                state.metadata_cancel = None;
                state.error = Some(format!("failed to fetch metadata: {error}"));
            }
        }
    }

    pub(super) fn handle_metadata(
        &mut self,
        pane: PaneId,
        generation: u64,
        result: Result<MetadataResult, IndexError>,
        sender: &ComponentSender<Self>,
    ) {
        let state = self.pane_mut(pane);
        if generation != state.generation {
            return;
        }
        state.metadata_inflight = false;
        state.metadata_cancel = None;
        match result {
            Ok(result) => {
                tracing::debug!(
                    pane = pane.label(),
                    elapsed_ms = result.elapsed.as_secs_f64() * 1_000.0,
                    pending = state.metadata_pending.len(),
                    "visible metadata batch complete"
                );
                if !result.failures.is_empty() {
                    tracing::debug!(
                        pane = pane.label(),
                        failures = result.failures.len(),
                        "visible metadata batch had raced entries"
                    );
                }
                state.metadata_pending.retain(|row| {
                    result
                        .listing
                        .source_index_at_row(*row as usize)
                        .is_some_and(|source_index| result.listing.metadata(source_index).is_none())
                });
                state.active_mut().listing = Some(result.listing);
                state.metadata_revision = state.metadata_revision.wrapping_add(1);
            }
            Err(IndexError::Cancelled) => return,
            Err(error) => state.error = Some(error.to_string()),
        }
        if !state.metadata_pending.is_empty() {
            self.start_metadata(pane, sender);
        }
    }

    pub(super) fn handle_frame_painted(
        &mut self,
        pane: PaneId,
        complete: bool,
        elapsed: Duration,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(acknowledge) = self.pane(pane).presentation_ack.as_ref() {
            let _ = acknowledge.try_send(());
        }
        if !self.profile_startup {
            return;
        }
        let index = pane.index();
        if self.first_paints[index].is_none() {
            self.first_paints[index] = Some(elapsed);
            tracing::info!(
                pane = pane.label(),
                elapsed_ms = elapsed.as_secs_f64() * 1_000.0,
                "first rows painted"
            );
        }
        if complete {
            self.complete_paints[index] = Some(elapsed);
        }

        if self.quit_after_first_paint && self.first_paints.iter().all(Option::is_some) {
            self.report_startup();
            relm4::main_application().quit();
        } else if self.benchmark_mode
            && self.complete_paints.iter().all(Option::is_some)
            && self.scroll_epoch == 0
        {
            if self.benchmark_filter.is_some() {
                self.maybe_start_filter_benchmark(sender);
            } else {
                if !self.startup_reported {
                    self.report_startup();
                    self.startup_reported = true;
                }
                self.scroll_epoch = 1;
            }
        }
    }

    pub(super) fn maybe_start_filter_benchmark(&mut self, sender: &ComponentSender<Self>) {
        if !self.benchmark_mode
            || self.filter_benchmark_started
            || self.filter_benchmark_scheduled
            || !self.complete_paints.iter().all(Option::is_some)
            || !self.pane(PaneId::Left).filter_ready
        {
            return;
        }
        let Some(query) = self.benchmark_filter.clone() else {
            return;
        };
        if !self.startup_reported {
            self.report_startup();
            self.startup_reported = true;
        }
        let _ = query;
        self.filter_benchmark_scheduled = true;
        let input = sender.input_sender().clone();
        glib::timeout_add_local_once(Duration::from_millis(750), move || {
            let _ = input.send(AppMsg::StartFilterBenchmark);
        });
    }

    pub(super) fn report_startup(&self) {
        let left = self.first_paints[0].unwrap_or_default().as_secs_f64() * 1_000.0;
        let right = self.first_paints[1].unwrap_or_default().as_secs_f64() * 1_000.0;
        println!("DUALPANE_STARTUP left_first_paint_ms={left:.3} right_first_paint_ms={right:.3}");
    }

    pub(super) fn measure_rss(&mut self, sender: &ComponentSender<Self>) {
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-rss".to_owned())
            .spawn(move || {
                let _ = input.send(AppMsg::RssMeasured(dualpane_platform::process_rss_kib()));
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                tracing::warn!(%error, "failed to start RSS measurement worker");
                let _ = sender.input_sender().send(AppMsg::RssMeasured(None));
            }
        }
    }

    pub(super) fn report_benchmark(&self, rss_kib: Option<u64>) {
        let left = self.scroll_results[0].unwrap_or_default();
        let right = self.scroll_results[1].unwrap_or_default();
        println!(
            "DUALPANE_SCROLL left_p95_frame_ms={:.3} left_dropped={} left_max_bind_us={:.3} \
             left_max_render_us={:.3} left_max_scroll_us={:.3} left_bound_rows={} \
             right_p95_frame_ms={:.3} right_dropped={} right_max_bind_us={:.3} \
             right_max_render_us={:.3} right_max_scroll_us={:.3} right_bound_rows={} rss_kib={}",
            left.p95_frame_ms,
            left.dropped_frames,
            left.max_bind_us,
            left.max_render_us,
            left.max_scroll_us,
            left.bound_rows,
            right.p95_frame_ms,
            right.dropped_frames,
            right.max_bind_us,
            right.max_render_us,
            right.max_scroll_us,
            right.bound_rows,
            rss_kib.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
        );
    }

    pub(super) fn persist_session(&self) {
        self.session_worker.save(SessionState {
            vertical_split: self.vertical_split,
            dual_pane: self.dual_pane,
            split_position: self.split_position,
            left: self.panes[0].to_session(),
            right: self.panes[1].to_session(),
            keymap_profile: self.keymap.profile(),
            sidebar_visible: self.sidebar_visible,
            preview_visible: self.preview_visible,
            preview_width: self.preview_width,
            bookmarks: self.bookmarks.iter().map(ToString::to_string).collect(),
            favorite_groups: self.favorite_groups.clone(),
            recent: self.recent.iter().map(ToString::to_string).collect(),
            window_width: self.window_width,
            window_height: self.window_height,
            workspaces: self.workspaces.clone(),
            remote_uris: self.remote_uris.clone(),
            appearance: self.appearance,
            color_theme: self.color_theme,
            parallel_transfers: self.parallel_transfers,
            custom_tools: self.custom_tools.clone(),
            tags: self.tags.clone(),
        });
    }

    pub(super) fn reap_aux_workers(&mut self) {
        let mut active = Vec::new();
        for worker in self.aux_workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                active.push(worker);
            }
        }
        self.aux_workers = active;
        self.panes[0].reap_workers();
        self.panes[1].reap_workers();
    }

    pub(super) fn push_operation_log(&mut self, line: String) {
        const MAX_LOG_LINES: usize = 200;
        if self.operation_log.len() == MAX_LOG_LINES {
            self.operation_log.pop_front();
        }
        let lower = line.to_ascii_lowercase();
        let status =
            if lower.contains("fail") || lower.contains("error") || lower.contains("denied") {
                OperationLogStatus::Error
            } else if lower.contains("cancel") || lower.contains("skip") {
                OperationLogStatus::Warning
            } else if lower.contains("finished")
                || lower.contains("created")
                || lower.contains("updated")
                || lower.contains("connected")
            {
                OperationLogStatus::Success
            } else {
                OperationLogStatus::Info
            };
        let label = operation_log_label(&line).to_owned();
        let id = self.next_operation_log_id;
        self.next_operation_log_id = self.next_operation_log_id.wrapping_add(1);
        self.operation_log.push_back(OperationLogEntry {
            id,
            created_at: SystemTime::now(),
            status,
            label,
            detail: line,
        });
        self.operation_log_revision = self.operation_log_revision.wrapping_add(1);
    }

    pub(super) fn on_start_filter_benchmark(&mut self, sender: &ComponentSender<Self>) {
        if !self.filter_benchmark_started
            && let Some(query) = self.benchmark_filter.clone()
        {
            self.filter_benchmark_started = true;
            self.filter_benchmark_started_at = [Some(Instant::now()), None];
            self.set_filter(PaneId::Left, query, sender);
        }
    }

    pub(super) fn on_scroll_finished(
        &mut self,
        pane: PaneId,
        metrics: ScrollMetrics,
        sender: &ComponentSender<Self>,
    ) {
        self.scroll_results[pane.index()] = Some(metrics);
        if pane == PaneId::Left && self.scroll_results[1].is_none() {
            self.scroll_epoch = 2;
        }
        if self.scroll_results.iter().all(Option::is_some) && !self.rss_pending {
            self.rss_pending = true;
            self.measure_rss(sender);
        }
    }
}
