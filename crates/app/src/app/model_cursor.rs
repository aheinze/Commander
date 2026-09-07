//! Cursor movement, type-ahead, and selection commands on the active pane.

use super::*;

impl PaneState {
    pub(super) fn selected_miller_sources(&self) -> Vec<VPath> {
        if self.selection.is_empty() {
            return Vec::new();
        }
        self.miller_columns
            .iter()
            .filter_map(|column| column.listing.as_ref())
            .flat_map(|listing| {
                listing
                    .rows()
                    .filter(|entry| {
                        self.selection
                            .contains(&SelectionKey::for_entry(listing.parent(), entry))
                    })
                    .map(|entry| listing.parent().join_name(entry.name()))
            })
            .collect()
    }

    pub(super) fn retain_miller_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        // Opening a child column keeps its folder selected in the parent.
        // Refreshes and filesystem updates must retain selections in that column too.
        let available = self
            .miller_columns
            .iter()
            .filter_map(|column| column.listing.as_ref())
            .flat_map(|listing| {
                listing
                    .rows()
                    .map(|entry| SelectionKey::for_entry(listing.parent(), entry))
            })
            .filter(|key| self.selection.contains(key))
            .collect();
        self.selection.retain_available(&available);
    }

    fn retain_miller_column_selection(&mut self, column: usize) {
        if self.selection.is_empty() {
            return;
        }
        let available = self.miller_columns[column]
            .listing
            .iter()
            .flat_map(|listing| {
                listing
                    .rows()
                    .map(|entry| SelectionKey::for_entry(listing.parent(), entry))
            })
            .filter(|key| self.selection.contains(key))
            .collect();
        self.selection.retain_available(&available);
    }

    pub(super) fn select_miller_rows(
        &mut self,
        column: usize,
        path: &VPath,
        selection: Selection,
        row: Option<u32>,
    ) -> bool {
        let Some(current) = self
            .miller_columns
            .get(column)
            .filter(|current| &current.path == path)
        else {
            return false;
        };
        let target = row.and_then(|row| {
            current
                .listing
                .as_ref()?
                .row(row as usize)
                .map(|entry| (path.join_name(entry.name()), entry.kind()))
        });
        if row.is_some() && target.is_none() {
            return false;
        }
        // Clicking an ancestor that is already open must not discard the rest
        // of the branch before its activation gets the chance to reuse it.
        if selection.len() == 1
            && self.miller_columns.get(column + 1).is_some_and(|child| {
                target
                    .as_ref()
                    .is_some_and(|(path, kind)| kind.is_directory() && child.path == *path)
            })
            && current
                .listing
                .as_ref()
                .zip(row)
                .is_some_and(|(listing, row)| {
                    listing.row(row as usize).is_some_and(|entry| {
                        selection.contains(&SelectionKey::for_entry(listing.parent(), entry))
                    })
                })
        {
            self.miller_columns[column].selected_row = row;
            self.miller_focus = target;
            self.selection = selection;
            self.selection_revision = self.selection_revision.wrapping_add(1);
            self.restore_names.clear();
            self.range_anchor = None;
            self.miller_revision = self.miller_revision.wrapping_add(1);
            return true;
        }
        if self.miller_columns.len() > column + 1 {
            if let Some(cancel) = self.miller_cancel.take() {
                cancel.cancel();
            }
            self.miller_generation = self.miller_generation.wrapping_add(1);
            self.remember_navigation();
            self.miller_columns.truncate(column + 1);
            self.loading = false;
            self.filtering = false;
            for column in &mut self.miller_columns {
                column.loading = false;
            }
        }
        self.miller_columns[column].selected_row = row;
        self.miller_focus = target;
        self.selection = selection;
        self.restore_names.clear();
        self.selection_revision = self.selection_revision.wrapping_add(1);
        self.range_anchor = None;
        self.miller_revision = self.miller_revision.wrapping_add(1);
        true
    }
}

impl AppModel {
    pub(super) fn focus_context_target(
        &mut self,
        pane: PaneId,
        target: Option<(VPath, EntryKind)>,
    ) {
        self.active_pane = pane;
        if self.pane(pane).view_mode == PaneViewMode::Columns {
            let state = self.pane_mut(pane);
            let Some((path, kind)) = target else {
                state.miller_focus = None;
                state.selection.clear();
                state.selection_revision = state.selection_revision.wrapping_add(1);
                state.miller_revision = state.miller_revision.wrapping_add(1);
                return;
            };
            let selected =
                state
                    .miller_columns
                    .iter()
                    .enumerate()
                    .find_map(|(column_index, column)| {
                        column.listing.as_ref().and_then(|listing| {
                            listing.rows().enumerate().find_map(|(row, entry)| {
                                (column.path.join_name(entry.name()) == path)
                                    .then_some((column_index, row as u32))
                            })
                        })
                    });
            if let Some((column_index, row)) = selected {
                let column = &state.miller_columns[column_index];
                let parent = column.path.clone();
                let listing = column.listing.as_ref().expect("located row has a listing");
                let key = SelectionKey::for_entry(
                    listing.parent(),
                    listing.row(row as usize).expect("located row exists"),
                );
                let selection = if column_index + 1 == state.miller_columns.len()
                    && state.selection.contains(&key)
                {
                    state.selection.clone()
                } else {
                    let mut selection = Selection::new();
                    selection.select(key);
                    selection
                };
                state.select_miller_rows(column_index, &parent, selection, Some(row));
            } else {
                state.miller_focus = Some((path, kind));
                state.miller_revision = state.miller_revision.wrapping_add(1);
            }
            return;
        }

        let row_and_key = target.as_ref().and_then(|(path, _)| {
            let listing = self.pane(pane).active().listing.as_ref()?;
            listing.rows().enumerate().find_map(|(row, entry)| {
                let entry_path = listing.parent().join_name(entry.name());
                (entry_path == *path)
                    .then(|| (row as u32, SelectionKey::for_entry(listing.parent(), entry)))
            })
        });
        let state = self.pane_mut(pane);
        if let Some((row, key)) = row_and_key {
            state.cursor_row = row;
            if !state.selection.contains(&key) {
                state.selection.replace([key]);
                state.selection_revision = state.selection_revision.wrapping_add(1);
            }
        } else if !state.selection.is_empty() {
            state.selection.clear();
            state.selection_revision = state.selection_revision.wrapping_add(1);
        }
    }

    pub(super) fn typeahead(&mut self, character: char) {
        const RESET_AFTER: Duration = Duration::from_millis(850);
        let pane = self.active_pane;
        let now = Instant::now();
        let state = self.pane_mut(pane);
        let expired = state
            .typeahead_updated
            .is_none_or(|updated| now.duration_since(updated) > RESET_AFTER);
        let repeated = !expired
            && state.typeahead_query.chars().count() == 1
            && state
                .typeahead_query
                .chars()
                .next()
                .is_some_and(|previous| previous.eq_ignore_ascii_case(&character));
        if expired {
            state.typeahead_query.clear();
        }
        if !repeated {
            state.typeahead_query.push(character);
        }
        state.typeahead_updated = Some(now);
        let query = state.typeahead_query.to_lowercase();
        let current = state.cursor_row as usize;
        let Some(listing) = state.active().listing.as_ref() else {
            return;
        };
        let length = listing.len();
        if length == 0 {
            return;
        }
        let start = if repeated {
            current.saturating_add(1) % length
        } else {
            current.min(length - 1)
        };
        let match_row = (0..length)
            .map(|offset| (start + offset) % length)
            .find(|&row| {
                listing.row(row).is_some_and(|entry| {
                    entry
                        .name()
                        .to_string_lossy()
                        .to_lowercase()
                        .starts_with(&query)
                })
            });
        if let Some(row) = match_row {
            state.cursor_row = u32::try_from(row).unwrap_or(u32::MAX);
            state.range_anchor = None;
        }
    }

    pub(super) fn move_cursor(&mut self, pane: PaneId, delta: i32, extend: bool) {
        let Some(length) = self
            .pane(pane)
            .active()
            .listing
            .as_ref()
            .map(|listing| listing.len())
        else {
            return;
        };
        if length == 0 {
            return;
        }
        let old = self.pane(pane).cursor_row;
        let last = u32::try_from(length - 1).unwrap_or(u32::MAX);
        let new = if delta < 0 {
            old.saturating_sub(delta.unsigned_abs())
        } else {
            old.saturating_add(delta as u32).min(last)
        };
        self.set_cursor(pane, new, extend);
    }

    pub(super) fn move_cursor_vertical(&mut self, pane: PaneId, delta: i32, extend: bool) {
        match self.pane(pane).view_mode {
            PaneViewMode::List => self.move_cursor(pane, delta, extend),
            PaneViewMode::Grid => {
                let stride = i32::try_from(self.pane(pane).grid_columns).unwrap_or(1);
                self.move_cursor(pane, delta.saturating_mul(stride), extend);
            }
            PaneViewMode::Columns => self.move_miller_cursor(pane, delta, extend),
        }
    }

    pub(super) fn move_cursor_horizontal(
        &mut self,
        pane: PaneId,
        delta: i32,
        extend: bool,
        sender: &ComponentSender<Self>,
    ) {
        match self.pane(pane).view_mode {
            PaneViewMode::Grid => self.move_cursor(pane, delta.signum(), extend),
            PaneViewMode::List if extend => self.move_cursor(pane, delta.signum(), true),
            PaneViewMode::List if delta < 0 => {
                let _ = sender.input_sender().send(AppMsg::Up(pane));
            }
            PaneViewMode::List => {
                if let Some((path, EntryKind::Directory)) = self.focused_item(pane) {
                    self.open_path(pane, EntryKind::Directory, path, sender);
                }
            }
            PaneViewMode::Columns if extend => {
                self.move_miller_cursor(pane, delta.signum(), true);
            }
            PaneViewMode::Columns if delta < 0 => self.move_miller_left(pane, sender),
            PaneViewMode::Columns => {
                let target = self
                    .pane(pane)
                    .miller_columns
                    .len()
                    .checked_sub(1)
                    .and_then(|column| {
                        self.pane(pane).miller_columns[column]
                            .selected_row
                            .map(|row| (column, row))
                    });
                if let Some((column, row)) = target {
                    self.open_miller_row(pane, column, row, sender);
                }
            }
        }
    }

    pub(super) fn move_miller_cursor(&mut self, pane: PaneId, delta: i32, extend: bool) {
        let Some(column) = self.pane(pane).miller_columns.len().checked_sub(1) else {
            return;
        };
        let Some(length) = self.pane(pane).miller_columns[column]
            .listing
            .as_ref()
            .map(|listing| listing.len())
        else {
            return;
        };
        if length == 0 {
            return;
        }
        let current = self.pane(pane).miller_columns[column]
            .selected_row
            .unwrap_or(0);
        let last = u32::try_from(length - 1).unwrap_or(u32::MAX);
        let new = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as u32).min(last)
        };
        self.set_miller_cursor(pane, column, new, extend);
    }

    pub(super) fn set_miller_cursor(
        &mut self,
        pane: PaneId,
        column: usize,
        new: u32,
        extend: bool,
    ) {
        let current = self.pane(pane).miller_columns[column]
            .selected_row
            .unwrap_or(new);
        let anchor = if extend {
            self.pane(pane).range_anchor.unwrap_or(current)
        } else {
            new
        };
        let (target, keys) = {
            let column_state = &self.pane(pane).miller_columns[column];
            let Some(listing) = column_state.listing.as_ref() else {
                return;
            };
            let target = listing
                .row(new as usize)
                .map(|entry| (column_state.path.join_name(entry.name()), entry.kind()));
            let keys = if extend {
                let start = anchor.min(new);
                let end = anchor.max(new);
                (start..=end)
                    .filter_map(|row| {
                        listing
                            .row(row as usize)
                            .map(|entry| SelectionKey::for_entry(listing.parent(), entry))
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            (target, keys)
        };
        let state = self.pane_mut(pane);
        if target.is_none() {
            return;
        }
        if state.miller_columns.len() > column + 1 {
            if let Some(cancel) = state.miller_cancel.take() {
                cancel.cancel();
            }
            state.miller_generation = state.miller_generation.wrapping_add(1);
        }
        state.miller_columns.truncate(column + 1);
        state.miller_columns[column].selected_row = Some(new);
        state.miller_focus = target;
        state.range_anchor = extend.then_some(anchor);
        if extend {
            state.selection.replace(keys);
            state.selection_revision = state.selection_revision.wrapping_add(1);
        } else if !state.selection.is_empty() {
            // Moving into a child column leaves its parent's selection behind,
            // while keeping marked items within the column being navigated.
            let before = state.selection.len();
            state.retain_miller_column_selection(column);
            if state.selection.len() != before {
                state.selection_revision = state.selection_revision.wrapping_add(1);
            }
        }
        state.restore_names.clear();
        state.miller_revision = state.miller_revision.wrapping_add(1);
    }

    pub(super) fn move_miller_left(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        if self.pane(pane).miller_columns.len() <= 1 {
            let _ = sender.input_sender().send(AppMsg::Up(pane));
            return;
        }
        let state = self.pane_mut(pane);
        if let Some(cancel) = state.miller_cancel.take() {
            cancel.cancel();
        }
        state.remember_navigation();
        state.miller_columns.pop();
        state.loading = false;
        state.filtering = false;
        state.filter_query.clear();
        for column in &mut state.miller_columns {
            column.loading = false;
        }
        state.selection.clear();
        state.selection_revision = state.selection_revision.wrapping_add(1);
        state.range_anchor = None;
        state.miller_generation = state.miller_generation.wrapping_add(1);
        state.miller_focus = state.miller_columns.last().and_then(|column| {
            let listing = column.listing.as_ref()?;
            let row = column.selected_row?;
            let entry = listing.row(row as usize)?;
            Some((column.path.join_name(entry.name()), entry.kind()))
        });
        state.miller_revision = state.miller_revision.wrapping_add(1);
        self.persist_session();
    }

    pub(super) fn move_cursor_to(&mut self, pane: PaneId, target: CursorTarget, extend: bool) {
        const PAGE_ROWS: u32 = 12;
        if self.pane(pane).view_mode == PaneViewMode::Columns {
            let Some(column) = self.pane(pane).miller_columns.len().checked_sub(1) else {
                return;
            };
            let Some(length) = self.pane(pane).miller_columns[column]
                .listing
                .as_ref()
                .map(|listing| listing.len())
            else {
                return;
            };
            if length == 0 {
                return;
            }
            let current = self.pane(pane).miller_columns[column]
                .selected_row
                .unwrap_or(0);
            let last = u32::try_from(length - 1).unwrap_or(u32::MAX);
            let new = match target {
                CursorTarget::First => 0,
                CursorTarget::Last => last,
                CursorTarget::PageUp => current.saturating_sub(PAGE_ROWS),
                CursorTarget::PageDown => current.saturating_add(PAGE_ROWS).min(last),
            };
            self.set_miller_cursor(pane, column, new, extend);
            return;
        }
        let Some(length) = self
            .pane(pane)
            .active()
            .listing
            .as_ref()
            .map(|listing| listing.len())
        else {
            return;
        };
        if length == 0 {
            return;
        }
        let current = self.pane(pane).cursor_row;
        let last = u32::try_from(length - 1).unwrap_or(u32::MAX);
        let page = if self.pane(pane).view_mode == PaneViewMode::Grid {
            PAGE_ROWS.saturating_mul(self.pane(pane).grid_columns)
        } else {
            PAGE_ROWS
        };
        let new = match target {
            CursorTarget::First => 0,
            CursorTarget::Last => last,
            CursorTarget::PageUp => current.saturating_sub(page),
            CursorTarget::PageDown => current.saturating_add(page).min(last),
        };
        self.set_cursor(pane, new, extend);
    }

    pub(super) fn set_cursor(&mut self, pane: PaneId, new: u32, extend: bool) {
        let old = self.pane(pane).cursor_row;
        let anchor = if extend {
            self.pane(pane).range_anchor.unwrap_or(old)
        } else {
            new
        };
        let keys = if extend {
            let start = anchor.min(new);
            let end = anchor.max(new);
            (start..=end)
                .filter_map(|row| self.selection_key_at(pane, row))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let state = self.pane_mut(pane);
        state.cursor_row = new;
        state.range_anchor = extend.then_some(anchor);
        if extend {
            state.selection.replace(keys);
            state.selection_revision = state.selection_revision.wrapping_add(1);
        }
    }

    pub(super) fn toggle_cursor(&mut self, pane: PaneId) {
        if self.pane(pane).view_mode == PaneViewMode::Columns {
            self.toggle_miller_cursor(pane);
            return;
        }
        let row = self.pane(pane).cursor_row;
        let Some(key) = self.selection_key_at(pane, row) else {
            return;
        };
        let length = self
            .pane(pane)
            .active()
            .listing
            .as_ref()
            .map_or(0, |listing| listing.len());
        let state = self.pane_mut(pane);
        state.selection.toggle(key);
        state.selection_revision = state.selection_revision.wrapping_add(1);
        state.range_anchor = None;
        if length > 0 {
            state.cursor_row = state
                .cursor_row
                .saturating_add(1)
                .min(u32::try_from(length - 1).unwrap_or(u32::MAX));
        }
    }

    pub(super) fn toggle_miller_cursor(&mut self, pane: PaneId) {
        let Some(column) = self.pane(pane).miller_columns.len().checked_sub(1) else {
            return;
        };
        let (key, next, next_target) = {
            let column_state = &self.pane(pane).miller_columns[column];
            let Some(listing) = column_state.listing.as_ref() else {
                return;
            };
            if listing.is_empty() {
                return;
            }
            let row = column_state.selected_row.unwrap_or(0);
            let Some(entry) = listing.row(row as usize) else {
                return;
            };
            let key = SelectionKey::for_entry(listing.parent(), entry);
            let last = u32::try_from(listing.len() - 1).unwrap_or(u32::MAX);
            let next = row.saturating_add(1).min(last);
            let next_target = listing
                .row(next as usize)
                .map(|entry| (column_state.path.join_name(entry.name()), entry.kind()));
            (key, next, next_target)
        };
        let state = self.pane_mut(pane);
        state.retain_miller_column_selection(column);
        state.restore_names.clear();
        state.selection.toggle(key);
        state.selection_revision = state.selection_revision.wrapping_add(1);
        state.range_anchor = None;
        state.miller_columns[column].selected_row = Some(next);
        state.miller_focus = next_target;
        state.miller_revision = state.miller_revision.wrapping_add(1);
    }

    pub(super) fn select_all(&mut self, pane: PaneId) {
        let keys = self
            .selection_listing(pane)
            .map(|listing| {
                listing
                    .rows()
                    .map(|entry| SelectionKey::for_entry(listing.parent(), entry))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let state = self.pane_mut(pane);
        state.selection.replace(keys);
        state.selection_revision = state.selection_revision.wrapping_add(1);
    }

    pub(super) fn clear_selection(&mut self, pane: PaneId) {
        let state = self.pane_mut(pane);
        if !state.selection.is_empty() {
            state.selection.clear();
            state.selection_revision = state.selection_revision.wrapping_add(1);
        }
        state.range_anchor = None;
    }

    pub(super) fn invert_selection(&mut self, pane: PaneId) {
        let keys = self
            .selection_listing(pane)
            .map(|listing| {
                listing
                    .rows()
                    .map(|entry| SelectionKey::for_entry(listing.parent(), entry))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let state = self.pane_mut(pane);
        state.selection.invert(keys);
        state.restore_names.clear();
        state.selection_revision = state.selection_revision.wrapping_add(1);
    }

    pub(super) fn on_selection_changed(
        &mut self,
        pane: PaneId,
        selection: Selection,
        cursor_row: Option<u32>,
        sender: &ComponentSender<Self>,
    ) {
        let state = self.pane_mut(pane);
        state.selection = selection;
        state.selection_revision = state.selection_revision.wrapping_add(1);
        if let Some(cursor_row) = cursor_row {
            state.cursor_row = cursor_row;
            state.range_anchor = None;
        }
        if pane == self.active_pane {
            self.start_preview(sender);
        }
    }

    pub(super) fn on_move_cursor(
        &mut self,
        delta: i32,
        extend: bool,
        sender: &ComponentSender<Self>,
    ) {
        if self.pane(self.active_pane).view_mode == PaneViewMode::Columns {
            self.move_miller_cursor(self.active_pane, delta, extend);
        } else {
            self.move_cursor(self.active_pane, delta, extend);
        }
        self.start_preview(sender);
    }
}
