//! View, sort, and settings state changed directly by the user.

use super::*;

impl AppModel {
    pub(super) fn on_move_palette_selection(&mut self, delta: i32) {
        let count = self.palette_items().len();
        if count > 0 {
            self.palette_selection = if delta < 0 {
                self.palette_selection
                    .saturating_sub(delta.unsigned_abs() as usize)
            } else {
                self.palette_selection
                    .saturating_add(delta as usize)
                    .min(count - 1)
            };
        }
    }

    pub(super) fn on_set_view_mode(&mut self, mode: PaneViewMode, sender: &ComponentSender<Self>) {
        let pane = self.active_pane;
        let was_columns = self.pane(pane).view_mode == PaneViewMode::Columns;
        let path = self.pane(pane).current_directory().clone();
        self.pane_mut(pane).remember_navigation();
        let reroot = mode != PaneViewMode::Columns && path != self.pane(pane).active().path;
        if reroot {
            self.pane_mut(pane).active_mut().navigate(path);
            self.pane_mut(pane).reset_directory_view();
        }
        self.pane_mut(pane).view_mode = mode;
        if mode == PaneViewMode::Columns {
            self.sync_miller_root(pane);
        }
        if reroot || (was_columns && mode != PaneViewMode::Columns) {
            self.start_listing(pane, sender);
        }
        self.focus_active_files();
        self.start_preview(sender);
        self.persist_session();
    }

    pub(super) fn on_set_sort(
        &mut self,
        pane: PaneId,
        key: SortKey,
        sender: &ComponentSender<Self>,
    ) {
        if self.pane(pane).sort.key != key {
            self.pane_mut(pane).sort.key = key;
            self.start_listing(pane, sender);
            self.persist_session();
        }
    }

    pub(super) fn on_toggle_sort_direction(
        &mut self,
        pane: PaneId,
        sender: &ComponentSender<Self>,
    ) {
        let state = self.pane_mut(pane);
        state.sort.direction = match state.sort.direction {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        };
        self.start_listing(pane, sender);
        self.persist_session();
    }

    pub(super) fn on_toggle_hidden_active(&mut self, sender: &ComponentSender<Self>) {
        let pane = self.active_pane;
        let state = self.pane_mut(pane);
        state.show_hidden = !state.show_hidden;
        if let Some(cancel) = state.filter_cancel.take() {
            cancel.cancel();
        }
        state.filter_sender = None;
        state.filter_ready = false;
        self.ensure_filter_worker(pane, sender);
        self.start_filter(pane, sender);
        self.persist_session();
    }

    pub(super) fn on_toggle_preview(&mut self, sender: &ComponentSender<Self>) {
        self.preview_visible = !self.preview_visible;
        self.refresh_grid_column_estimates();
        if self.preview_visible {
            self.start_preview(sender);
        }
        self.persist_session();
    }

    pub(super) fn on_preview_width(&mut self, width: i32) {
        if self.preview_visible {
            let width = width.clamp(PREVIEW_MIN_WIDTH, PREVIEW_MAX_WIDTH);
            if width != self.preview_width {
                self.preview_width = width;
                self.persist_session();
            }
        }
    }

    pub(super) fn on_window_size(&mut self, width: i32, height: i32) {
        if width > 0 && height > 0 && (width != self.window_width || height != self.window_height) {
            self.window_width = width;
            self.window_height = height;
            self.refresh_grid_column_estimates();
            self.persist_session();
        }
    }

    pub(super) fn on_set_settings(
        &mut self,
        mut settings: settings::SettingsDraft,
        sender: &ComponentSender<Self>,
    ) {
        settings.workflow.recent_limit = settings.workflow.recent_limit.clamp(5, 100);
        let sort_changed = self.workflow.directories_first != settings.workflow.directories_first;
        let inspector_changed = self.workflow.inspector_folder_sizes
            != settings.workflow.inspector_folder_sizes
            || self.workflow.inspector_git != settings.workflow.inspector_git;
        let shortcuts_changed = self.keymap.overrides() != settings.overrides;
        self.appearance = settings.appearance;
        self.color_theme = settings.color_theme;
        self.parallel_transfers = settings.parallel_transfers;
        self.workflow = settings.workflow;
        self.keymap
            .configure(settings.profile, settings.overrides.clone());
        if shortcuts_changed {
            let input = sender.input_sender().clone();
            self.session_worker
                .save_keymap(settings.overrides, move |result| {
                    if let Err(error) = result {
                        let _ = input.send(AppMsg::SettingsSaveFailed(error));
                    }
                });
        }
        if settings.clear_recent || !self.workflow.remember_recent {
            self.recent.clear();
        }
        self.recent.truncate(self.workflow.recent_limit as usize);
        apply_appearance(self.appearance);
        apply_color_theme(self.color_theme, self.appearance);
        if sort_changed {
            for pane in [PaneId::Left, PaneId::Right] {
                self.pane_mut(pane).sort.directories_first = self.workflow.directories_first;
                self.start_listing(pane, sender);
            }
        }
        if inspector_changed {
            self.preview_state.cancel();
            self.preview_state.path = None;
            self.preview_state.content = None;
            self.folder_measure.reset();
            self.inspector_git.reset();
            self.start_preview(sender);
        }
        self.persist_session();
    }
}
