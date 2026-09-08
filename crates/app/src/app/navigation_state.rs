use super::*;

impl PaneState {
    /// Resolve by path only after the final sorted listing has arrived.
    pub(super) fn reveal_pending_item(&mut self) {
        if self.loading || self.filtering {
            return;
        }
        let Some(target) = self.pending_reveal.take() else {
            return;
        };
        let listing = if self.view_mode == PaneViewMode::Columns {
            self.miller_columns
                .last()
                .and_then(|column| column.listing.clone())
        } else {
            self.active().listing.clone()
        };
        let Some(listing) = listing.filter(|listing| listing.is_complete()) else {
            return;
        };
        if target.parent().as_ref() != Some(listing.parent()) {
            return;
        }
        let Some((row, entry)) = listing
            .rows()
            .enumerate()
            .find(|(_, entry)| Some(entry.name()) == target.file_name())
        else {
            self.error = Some("The search result is no longer in this folder.".to_owned());
            return;
        };
        self.cursor_row = row as u32;
        self.range_anchor = None;
        self.selection
            .replace([SelectionKey::for_entry(listing.parent(), entry)]);
        self.selection_revision = self.selection_revision.wrapping_add(1);
        if self.view_mode == PaneViewMode::Columns {
            if let Some(column) = self.miller_columns.last_mut() {
                column.selected_row = Some(row as u32);
            }
            self.miller_focus = Some((target, entry.kind()));
            self.miller_revision = self.miller_revision.wrapping_add(1);
        }
        self.reveal_epoch = self.reveal_epoch.wrapping_add(1);
    }

    fn view_snapshot(&self, width: i32, scroll_y: u32, name: Option<String>) -> FolderViewSession {
        FolderViewSession {
            view_mode: self.view_mode,
            sort_key: match self.sort.key {
                SortKey::Name => PaneSortKey::Name,
                SortKey::Size => PaneSortKey::Size,
                SortKey::Modified => PaneSortKey::Modified,
                SortKey::Kind => PaneSortKey::Type,
            },
            sort_descending: self.sort.direction == SortDirection::Descending,
            show_hidden: self.show_hidden,
            column_width: width,
            scroll_y,
            cursor_name: name,
        }
    }

    pub(super) fn navigation_snapshot(
        &self,
    ) -> (
        BTreeMap<String, FolderViewSession>,
        BTreeMap<String, NavigationSession>,
    ) {
        let mut folders = self.folder_views.clone();
        let mut locations = self.locations.clone();
        let listing = if self.view_mode == PaneViewMode::Columns {
            self.miller_columns
                .last()
                .and_then(|column| column.listing.as_ref())
        } else {
            self.active().listing.as_ref()
        };
        // Do not replace a saved selection with the transient empty loading state.
        let selected_names = listing.map_or_else(
            || self.restore_names.clone(),
            |listing| {
                listing
                    .rows()
                    .filter(|entry| {
                        self.selection
                            .contains(&SelectionKey::for_entry(listing.parent(), entry))
                    })
                    .filter_map(|entry| entry.name().to_str().map(str::to_owned))
                    .collect()
            },
        );
        if self.view_mode == PaneViewMode::Columns {
            for column in &self.miller_columns {
                let name = column
                    .selected_row
                    .and_then(|row| {
                        column
                            .listing
                            .as_ref()?
                            .row(row as usize)?
                            .name()
                            .to_str()
                            .map(str::to_owned)
                    })
                    .or_else(|| column.restore_name.as_ref()?.to_str().map(str::to_owned));
                folders.insert(
                    column.path.to_string(),
                    self.view_snapshot(column.width, column.scroll_y, name),
                );
            }
        } else {
            let name = self
                .active()
                .listing
                .as_ref()
                .and_then(|listing| {
                    listing
                        .row(self.cursor_row as usize)?
                        .name()
                        .to_str()
                        .map(str::to_owned)
                })
                .or_else(|| self.restore_cursor.clone());
            let width = folders
                .get(&self.active().path.to_string())
                .map_or(280, |folder| folder.column_width);
            folders.insert(
                self.active().path.to_string(),
                self.view_snapshot(width, self.scroll_y, name),
            );
        }
        locations.insert(
            self.active().path.to_string(),
            NavigationSession {
                columns: self
                    .miller_columns
                    .iter()
                    .map(|column| column.path.to_string())
                    .collect(),
                selected_names,
                horizontal_scroll: self.miller_scroll_x,
            },
        );
        (folders, locations)
    }

    pub(super) fn remember_navigation(&mut self) {
        (self.folder_views, self.locations) = self.navigation_snapshot();
    }

    pub(super) fn restore_navigation(&mut self) {
        let root = self.active().path.clone();
        if let Some(saved) = self.folder_views.get(&root.to_string()) {
            // The pane owns the view mode; folder history must not override the user's choice.
            self.sort.key = match saved.sort_key {
                PaneSortKey::Name => SortKey::Name,
                PaneSortKey::Size => SortKey::Size,
                PaneSortKey::Modified => SortKey::Modified,
                PaneSortKey::Type => SortKey::Kind,
            };
            self.sort.direction = if saved.sort_descending {
                SortDirection::Descending
            } else {
                SortDirection::Ascending
            };
            self.show_hidden = saved.show_hidden;
            self.scroll_y = saved.scroll_y;
            self.restore_cursor = saved.cursor_name.clone();
        } else {
            self.scroll_y = 0;
            self.restore_cursor = None;
        }
        let navigation = self
            .locations
            .get(&root.to_string())
            .cloned()
            .unwrap_or_default();
        self.restore_names = navigation.selected_names;
        self.miller_scroll_x = navigation.horizontal_scroll;
        self.scroll_restore_epoch = self.scroll_restore_epoch.wrapping_add(1);
        if self.view_mode == PaneViewMode::Columns {
            let mut paths = vec![root];
            for path in navigation
                .columns
                .into_iter()
                .skip(1)
                .take(63)
                .map(|path| VPath::from(path.as_str()))
            {
                if path.parent().as_ref() != paths.last() {
                    break;
                }
                paths.push(path);
            }
            self.miller_columns = paths
                .into_iter()
                .map(|path| {
                    let saved = self.folder_views.get(&path.to_string());
                    MillerColumnState {
                        width: saved.map_or(280, |s| {
                            if s.column_width == 0 {
                                280
                            } else {
                                s.column_width.clamp(220, 600)
                            }
                        }),
                        scroll_y: saved.map_or(0, |s| s.scroll_y),
                        restore_name: saved
                            .and_then(|s| s.cursor_name.as_ref())
                            .map(OsString::from),
                        path,
                        listing: None,
                        base_listing: None,
                        selected_row: None,
                        loading: true,
                        error: None,
                    }
                })
                .collect();
        }
    }

    pub(super) fn restore_selection(&mut self) {
        let listing = if self.view_mode == PaneViewMode::Columns {
            self.miller_columns
                .last()
                .and_then(|column| column.listing.as_ref())
        } else {
            self.active().listing.as_ref()
        };
        let Some(listing) = listing.filter(|listing| listing.is_complete()).cloned() else {
            return;
        };
        if let Some(name) = self.restore_cursor.take() {
            self.cursor_row = listing
                .rows()
                .position(|entry| entry.name() == OsStr::new(&name))
                .unwrap_or(0) as u32;
        }
        if !self.restore_names.is_empty() {
            let names: HashSet<&str> = self.restore_names.iter().map(String::as_str).collect();
            self.selection.replace(
                listing
                    .rows()
                    .filter(|entry| {
                        entry
                            .name()
                            .to_str()
                            .is_some_and(|name| names.contains(name))
                    })
                    .map(|entry| SelectionKey::for_entry(listing.parent(), entry)),
            );
            self.restore_names.clear();
            self.selection_revision = self.selection_revision.wrapping_add(1);
        }
    }
}
