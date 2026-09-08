//! Inspector data sources: previews, selection summaries, folder sizes, and Git context.

use super::*;

impl AppModel {
    pub(super) fn focused_path(&self, pane: PaneId) -> Option<VPath> {
        self.focused_item(pane).map(|(path, _)| path)
    }

    pub(super) fn focused_item(&self, pane: PaneId) -> Option<(VPath, EntryKind)> {
        if let Some(target) = &self.folder_action_target
            && target.pane == pane
        {
            return Some((target.path.clone(), EntryKind::Directory));
        }
        let state = self.pane(pane);
        if state.view_mode == PaneViewMode::Columns {
            if state.selection.len() == 1 {
                return state
                    .miller_columns
                    .iter()
                    .filter_map(|column| column.listing.as_ref())
                    .find_map(|listing| {
                        listing
                            .rows()
                            .find(|entry| {
                                state
                                    .selection
                                    .contains(&SelectionKey::for_entry(listing.parent(), entry))
                            })
                            .map(|entry| (listing.parent().join_name(entry.name()), entry.kind()))
                    });
            }
            return state.miller_focus.clone();
        }
        let listing = state.active().listing.as_ref()?;
        if state.selection.len() == 1 {
            if let Some(entry) = listing.row(state.cursor_row as usize)
                && state
                    .selection
                    .contains(&SelectionKey::for_entry(listing.parent(), entry))
            {
                return Some((listing.parent().join_name(entry.name()), entry.kind()));
            }
            if let Some(SelectionKey::Path(path)) = state.selection.anchor() {
                return Some((path.clone(), EntryKind::Unknown));
            }
            return None;
        }
        listing
            .row(state.cursor_row as usize)
            .map(|entry| (listing.parent().join_name(entry.name()), entry.kind()))
    }

    pub(super) fn start_preview(&mut self, sender: &ComponentSender<Self>) {
        if self.folder_action_target.is_none() && self.pane(self.active_pane).selection.len() > 1 {
            self.preview_state.cancel();
            self.preview_state.path = None;
            self.preview_state.content = None;
            self.preview_state.error = None;
            self.inspector_git.reset();
            self.folder_measure.reset();
            self.start_selection_summary(sender);
            return;
        }
        self.inspector_selection.reset();
        let Some((path, kind)) = self.focused_item(self.active_pane) else {
            self.preview_state.cancel();
            self.preview_state.path = None;
            self.preview_state.content = None;
            self.preview_state.error = None;
            self.inspector_git.reset();
            self.folder_measure.reset();
            return;
        };
        if self.preview_state.path.as_ref() == Some(&path)
            && (self.preview_state.loading || self.preview_state.content.is_some())
        {
            return;
        }
        let first_preview = self.preview_state.path.is_none();
        self.preview_state.cancel();
        self.folder_measure.reset();
        self.preview_state.generation = self.preview_state.generation.wrapping_add(1);
        self.preview_state.path = Some(path.clone());
        self.preview_state.content = None;
        self.preview_state.error = None;
        self.preview_state.loading = true;
        let generation = self.preview_state.generation;
        if kind == EntryKind::Directory {
            self.start_folder_measurement(vec![path.clone()], sender);
        }
        let input = sender.input_sender().clone();
        let delay = if first_preview {
            Duration::ZERO
        } else {
            PREVIEW_SETTLE_DELAY
        };
        glib::timeout_add_local_once(delay, move || {
            let _ = input.send(AppMsg::LoadPreview { generation, path });
        });
    }

    pub(super) fn begin_preview_load(
        &mut self,
        generation: u64,
        path: VPath,
        sender: &ComponentSender<Self>,
    ) {
        if generation != self.preview_state.generation
            || self.preview_state.path.as_ref() != Some(&path)
        {
            return;
        }
        let cancel = CancelToken::new();
        self.preview_state.cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        self.start_git_info(path.clone(), sender);
        match thread::Builder::new()
            .name("dualpane-preview".to_owned())
            .spawn(move || {
                let result = load_preview(vfs.as_ref(), path.clone(), &cancel);
                let _ = input.send(AppMsg::PreviewReady {
                    generation,
                    path,
                    result,
                });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.preview_state.loading = false;
                self.preview_state.cancel = None;
                self.preview_state.error = Some(format!("Could not start preview: {error}"));
            }
        }
    }

    pub(super) fn start_selection_summary(&mut self, sender: &ComponentSender<Self>) {
        let state = self.pane(self.active_pane);
        let Some(listing) = state.active().listing.clone() else {
            self.inspector_selection.reset();
            return;
        };
        let selection = state.selection.clone();
        self.inspector_selection.generation = self.inspector_selection.generation.wrapping_add(1);
        let generation = self.inspector_selection.generation;
        self.inspector_selection.loading = true;
        self.inspector_selection.summary = None;
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-selection-summary".to_owned())
            .spawn(move || {
                let mut summary = SelectionSummary {
                    count: selection.len(),
                    location: Some(listing.parent().clone()),
                    ..SelectionSummary::default()
                };
                for row in 0..listing.len() {
                    let Some(source_index) = listing.source_index_at_row(row) else {
                        continue;
                    };
                    let Some(entry) = listing.entry(source_index) else {
                        continue;
                    };
                    if !selection.contains(&SelectionKey::for_entry(listing.parent(), entry)) {
                        continue;
                    }
                    match entry.kind() {
                        EntryKind::File => summary.files += 1,
                        EntryKind::Directory => {
                            summary.folders += 1;
                            summary
                                .folder_paths
                                .push(listing.parent().join_name(entry.name()));
                        }
                        _ => summary.other += 1,
                    }
                    if entry.kind() != EntryKind::Directory {
                        if let Some(metadata) = listing.metadata(source_index) {
                            summary.known_bytes = summary.known_bytes.saturating_add(metadata.size);
                        } else {
                            summary.unknown_sizes += 1;
                        }
                    }
                }
                let _ = input.send(AppMsg::SelectionSummaryReady {
                    generation,
                    summary,
                });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.inspector_selection.loading = false;
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not summarize selection: {error}"));
            }
        }
    }

    pub(super) fn start_folder_measurement(
        &mut self,
        paths: Vec<VPath>,
        sender: &ComponentSender<Self>,
    ) {
        self.folder_measure.reset();
        if paths.is_empty() || !self.workflow.inspector_folder_sizes {
            return;
        }
        self.folder_measure.loading = true;
        let generation = self.folder_measure.generation;
        let cancel = CancelToken::new();
        self.folder_measure.cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-folder-measure".to_owned())
            .spawn(move || {
                let result = measure_folder_paths(vfs.as_ref(), &paths, &cancel);
                let _ = input.send(AppMsg::FolderMeasureReady { generation, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.folder_measure.loading = false;
                self.folder_measure.cancel = None;
                self.folder_measure.error = Some(format!("Could not measure folder: {error}"));
            }
        }
    }

    pub(super) fn start_git_info(&mut self, path: VPath, sender: &ComponentSender<Self>) {
        if !self.workflow.inspector_git {
            self.inspector_git.reset();
            return;
        }
        self.inspector_git.reset();
        self.inspector_git.loading = true;
        let generation = self.inspector_git.generation;
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-git-inspector".to_owned())
            .spawn(move || {
                let info = git_info_for_path(&path);
                let _ = input.send(AppMsg::GitInfoReady { generation, info });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(_) => {
                self.inspector_git.loading = false;
            }
        }
    }

    pub(super) fn render_pdf_preview(
        &mut self,
        page_number: usize,
        scale: f32,
        sender: &ComponentSender<Self>,
    ) {
        let Some(preview) = self.preview_state.content.as_ref() else {
            return;
        };
        let PreviewPayload::Pdf {
            bytes, page_count, ..
        } = &preview.payload
        else {
            return;
        };
        let page_number = page_number.clamp(1, *page_count);
        let bytes = Arc::clone(bytes);
        let generation = self.preview_state.generation;
        let path = preview.path.clone();
        let cancel = CancelToken::new();
        self.preview_state.cancel();
        self.preview_state.cancel = Some(cancel.clone());
        self.preview_state.loading = true;
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-pdf-preview".to_owned())
            .spawn(move || {
                let result = render_pdf_page(&bytes, page_number - 1, scale, &cancel);
                let _ = input.send(AppMsg::PdfRendered {
                    generation,
                    path,
                    page_number,
                    scale,
                    result,
                });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.preview_state.loading = false;
                self.preview_state.cancel = None;
                self.preview_state.error = Some(format!("Could not render PDF page: {error}"));
            }
        }
    }

    pub(super) fn on_pdf_ready(
        &mut self,
        result: Result<Vec<VPath>, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.tool_cancel = None;
        match result {
            Ok(outputs) => self
                .push_operation_log(format!("PDF operation created {} output(s)", outputs.len())),
            Err(error) => self.pane_mut(self.active_pane).error = Some(error),
        }
        self.start_listing(PaneId::Left, sender);
        self.start_listing(PaneId::Right, sender);
    }

    pub(super) fn on_preview_ready(
        &mut self,
        generation: u64,
        path: VPath,
        result: Result<Preview, PreviewError>,
        sender: &ComponentSender<Self>,
    ) {
        if generation == self.preview_state.generation
            && self.preview_state.path.as_ref() == Some(&path)
        {
            self.preview_state.loading = false;
            self.preview_state.cancel = None;
            match result {
                Ok(preview) => {
                    let measure_path = (preview.metadata.kind == EntryKind::Directory
                        && !self.folder_measure.loading)
                        .then(|| preview.path.clone());
                    self.preview_state.content = Some(preview);
                    self.preview_state.error = None;
                    if let Some(path) = measure_path {
                        self.start_folder_measurement(vec![path], sender);
                    }
                }
                Err(PreviewError::Cancelled) => {}
                Err(error) => {
                    self.preview_state.content = None;
                    self.preview_state.error = Some(error.to_string());
                }
            }
        }
    }

    pub(super) fn on_pdf_navigate(&mut self, delta: i32, sender: &ComponentSender<Self>) {
        if let Some(Preview {
            payload:
                PreviewPayload::Pdf {
                    page_number,
                    page_count,
                    scale,
                    ..
                },
            ..
        }) = self.preview_state.content.as_ref()
        {
            let page =
                (*page_number as i64 + i64::from(delta)).clamp(1, *page_count as i64) as usize;
            self.render_pdf_preview(page, *scale, sender);
        }
    }

    pub(super) fn on_pdf_zoom(&mut self, delta: i32, sender: &ComponentSender<Self>) {
        if let Some(Preview {
            payload: PreviewPayload::Pdf {
                page_number, scale, ..
            },
            ..
        }) = self.preview_state.content.as_ref()
        {
            let next = (*scale + delta as f32 * 0.15).clamp(PDF_MIN_SCALE, PDF_MAX_SCALE);
            self.render_pdf_preview(*page_number, next, sender);
        }
    }

    pub(super) fn on_pdf_fit(&mut self, sender: &ComponentSender<Self>) {
        let fit = self.preview_state.content.as_ref().and_then(|preview| {
            let PreviewPayload::Pdf {
                width,
                height,
                page_number,
                scale,
                ..
            } = &preview.payload
            else {
                return None;
            };
            Some((
                *page_number,
                fitted_pdf_scale(*width, *height, *scale, self.preview_width),
            ))
        });
        if let Some((page_number, scale)) = fit {
            self.render_pdf_preview(page_number, scale, sender);
        }
    }

    pub(super) fn on_pdf_rendered(
        &mut self,
        generation: u64,
        path: VPath,
        page_number: usize,
        scale: f32,
        result: Result<dualpane_thumbs::PdfPage, PreviewError>,
    ) {
        if generation == self.preview_state.generation
            && self.preview_state.path.as_ref() == Some(&path)
        {
            self.preview_state.loading = false;
            self.preview_state.cancel = None;
            match result {
                Ok(page) => {
                    if let Some(Preview {
                        payload:
                            PreviewPayload::Pdf {
                                rgba,
                                width,
                                height,
                                page_count,
                                page_number: current_page,
                                scale: current_scale,
                                ..
                            },
                        ..
                    }) = self.preview_state.content.as_mut()
                    {
                        *width = page.width();
                        *height = page.height();
                        *page_count = page.page_count();
                        *rgba = page.into_rgba();
                        *current_page = page_number;
                        *current_scale = scale;
                    }
                    self.preview_state.error = None;
                }
                Err(PreviewError::Cancelled) => {}
                Err(error) => self.preview_state.error = Some(error.to_string()),
            }
        }
    }

    pub(super) fn on_selection_summary_ready(
        &mut self,
        generation: u64,
        summary: SelectionSummary,
        sender: &ComponentSender<Self>,
    ) {
        if generation == self.inspector_selection.generation {
            self.inspector_selection.loading = false;
            let folder_paths = summary.folder_paths.clone();
            self.inspector_selection.summary = Some(summary);
            self.start_folder_measurement(folder_paths, sender);
        }
    }

    pub(super) fn on_folder_measure_ready(
        &mut self,
        generation: u64,
        result: Result<FolderMeasureResult, String>,
    ) {
        if generation == self.folder_measure.generation {
            self.folder_measure.loading = false;
            self.folder_measure.cancel = None;
            match result {
                Ok(result) => {
                    self.folder_measure.result = Some(result);
                    self.folder_measure.error = None;
                }
                Err(error) => {
                    self.folder_measure.result = None;
                    self.folder_measure.error = Some(error);
                }
            }
        }
    }

    pub(super) fn on_request_thumbnail(&mut self, pane: PaneId, generation: u64, path: VPath) {
        if self.pane(pane).generation != generation {
            return;
        }
        self.thumbnail_request_id = self.thumbnail_request_id.wrapping_add(1);
        let request = ThumbnailRequest {
            id: self.thumbnail_request_id,
            path: path.clone(),
            max_edge: GRID_THUMBNAIL_EDGE,
            cancel: self.pane(pane).thumbnail_cancel.clone(),
        };
        let scheduled = self
            .thumbnail_scheduler
            .as_ref()
            .is_some_and(|scheduler| scheduler.try_schedule(request).is_ok());
        if scheduled {
            self.thumbnail_outstanding
                .entry(path)
                .or_default()
                .insert(self.thumbnail_request_id);
        } else {
            self.thumbnail_revision = self.thumbnail_revision.wrapping_add(1);
            self.thumbnail_event = Some(ThumbnailUiEvent {
                path,
                thumbnail: None,
            });
        }
    }

    pub(super) fn on_thumbnail_ready(&mut self, response: ThumbnailResponse) {
        let ThumbnailResponse { id, path, result } = response;
        let known_request = self
            .thumbnail_outstanding
            .get_mut(&path)
            .is_some_and(|requests| requests.remove(&id));
        if !known_request {
            return;
        }
        match result {
            Ok(thumbnail) => {
                let newest_applied = self
                    .thumbnail_applied
                    .get(&path)
                    .is_some_and(|applied| *applied >= id);
                if !newest_applied {
                    self.thumbnail_applied.insert(path.clone(), id);
                    self.thumbnail_revision = self.thumbnail_revision.wrapping_add(1);
                    self.thumbnail_event = Some(ThumbnailUiEvent {
                        path: path.clone(),
                        thumbnail: Some(thumbnail),
                    });
                }
            }
            Err(ThumbnailError::Cancelled) => {}
            Err(error) => {
                tracing::debug!(%path, %error, "thumbnail generation failed");
            }
        }
        if self
            .thumbnail_outstanding
            .get(&path)
            .is_none_or(BTreeSet::is_empty)
        {
            self.thumbnail_outstanding.remove(&path);
            self.thumbnail_applied.remove(&path);
        }
    }
}
