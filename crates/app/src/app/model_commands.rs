//! Command dispatch: one handler per keyboard command or palette entry.

use super::*;

impl AppModel {
    pub(super) fn on_cycle_tab(&mut self, command: CommandId) -> Option<AppMsg> {
        let pane = self.active_pane;
        let state = self.pane(pane);
        let count = state.tabs.len();
        if count > 1 {
            let next = if command == CommandId::PreviousTab {
                state.active_tab.checked_sub(1).unwrap_or(count - 1)
            } else {
                (state.active_tab + 1) % count
            };
            Some(AppMsg::SelectTab(pane, next))
        } else {
            None
        }
    }

    pub(super) fn on_bookmark(&mut self) -> Option<AppMsg> {
        let path = self
            .archive_mounts
            .display(self.pane(self.active_pane).current_directory());
        if !self.bookmarks.contains(&path) {
            self.bookmarks.push(path);
            self.persist_session();
        }
        None
    }

    pub(super) fn on_swap_panes(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        self.panes[0].cancel_work();
        self.panes[1].cancel_work();
        self.panes.swap(0, 1);
        self.start_listing(PaneId::Left, sender);
        self.start_listing(PaneId::Right, sender);
        self.persist_session();
        None
    }

    pub(super) fn on_focus_sidebar(&mut self) -> Option<AppMsg> {
        self.sidebar_visible = true;
        self.refresh_grid_column_estimates();
        self.focus_sidebar_epoch = self.focus_sidebar_epoch.wrapping_add(1);
        self.persist_session();
        None
    }

    pub(super) fn on_focus_inspector(&mut self) -> Option<AppMsg> {
        self.preview_visible = true;
        self.refresh_grid_column_estimates();
        self.focus_inspector_epoch = self.focus_inspector_epoch.wrapping_add(1);
        self.persist_session();
        None
    }

    pub(super) fn on_command_palette(&mut self) -> Option<AppMsg> {
        self.palette_open = !self.palette_open;
        self.palette_selection = 0;
        if !self.palette_open {
            self.palette_query.clear();
            self.focus_active_files();
        }
        None
    }

    pub(super) fn on_rename(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        let sources = self.operation_sources(self.active_pane);
        if let [source] = sources.as_slice() {
            show_rename_dialog(source.clone(), sender);
        } else {
            self.pane_mut(self.active_pane).error =
                Some("Select exactly one item to rename".to_owned());
        }
        None
    }

    pub(super) fn on_batch_rename(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        let sources = self.operation_sources(self.active_pane);
        if sources.is_empty() {
            self.pane_mut(self.active_pane).error =
                Some("Select one or more items to rename".to_owned());
        } else {
            show_batch_rename_dialog(sources, sender);
        }
        None
    }

    pub(super) fn on_open_in_new_tab(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        if let Some((path, kind)) = self.focused_item(self.active_pane)
            && (kind == EntryKind::Directory || is_archive_path(&path))
        {
            let pane = self.active_pane;
            let state = self.pane_mut(pane);
            state.remember_navigation();
            state.tabs.push(TabState::new(path));
            state.active_tab = state.tabs.len() - 1;
            state.reset_for_folder_entry();
            state.tabs_revision = state.tabs_revision.wrapping_add(1);
            self.start_listing(pane, sender);
            self.persist_session();
        }
        None
    }

    pub(super) fn on_open_other_pane(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        if let Some((path, kind)) = self.focused_item(self.active_pane) {
            if kind != EntryKind::Directory && is_archive_path(&path) {
                let other = self.active_pane.other();
                self.active_pane = other;
                self.start_archive_browse(other, path, sender);
                return None;
            }
            let destination = if kind == EntryKind::Directory {
                path
            } else {
                path.parent().unwrap_or(path)
            };
            let other = self.active_pane.other();
            self.navigate(other, destination, sender);
            self.active_pane = other;
            self.focus_active_files();
        }
        None
    }

    pub(super) fn on_copy_path(&mut self) -> Option<AppMsg> {
        let paths = self.operation_sources(self.active_pane);
        if let Some(display) = gdk::Display::default()
            && !paths.is_empty()
        {
            display.clipboard().set_text(
                &paths
                    .iter()
                    .map(|path| self.archive_mounts.display(path).to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        None
    }

    pub(super) fn on_copy_directory_path(&mut self) -> Option<AppMsg> {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(
                &self
                    .archive_mounts
                    .display(self.pane(self.active_pane).current_directory())
                    .to_string(),
            );
        }
        None
    }

    pub(super) fn on_copy_or_cut(&mut self, command: CommandId) -> Option<AppMsg> {
        let paths = self.operation_sources(self.active_pane);
        self.copy_paths_to_clipboard(&paths, command == CommandId::Cut);
        None
    }

    pub(super) fn copy_paths_to_clipboard(&mut self, paths: &[VPath], cut: bool) {
        if let Some(display) = gdk::Display::default()
            && let Some(provider) = clipboard::provider(paths, cut)
        {
            match display.clipboard().set_content(Some(&provider)) {
                Ok(()) => {
                    self.clipboard_generation = self.clipboard_generation.wrapping_add(1);
                    self.clipboard_provider = Some(provider);
                }
                Err(error) => {
                    self.pane_mut(self.active_pane).error =
                        Some(format!("Could not copy files to the clipboard: {error}"))
                }
            }
        }
    }

    pub(super) fn on_delete_permanent(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        if self
            .operation_sources(self.active_pane)
            .iter()
            .any(|path| self.is_archive_browse_path(path))
        {
            self.review_archive_removal(sender);
            return None;
        }
        let count = self.operation_sources(self.active_pane).len();
        if count == 0 {
            self.pane_mut(self.active_pane).error =
                Some("No item is available to delete".to_owned());
        } else {
            show_permanent_delete_dialog(count, self.folder_action_input(sender));
        }
        None
    }

    pub(super) fn on_permissions(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        if let Some(target) = self.folder_action_target.clone() {
            let vfs = Arc::clone(&self.vfs);
            let input = sender.input_sender().clone();
            match thread::Builder::new()
                .name("dualpane-folder-permissions".to_owned())
                .spawn(move || {
                    let result = vfs
                        .stat(&target.path, false)
                        .map_err(|error| format!("Could not read folder permissions: {error}"))
                        .and_then(|metadata| {
                            metadata
                                .mode
                                .ok_or_else(|| "Folder permissions are unavailable".to_owned())
                        });
                    let _ = input.send(AppMsg::PermissionsInspected {
                        pane: target.pane,
                        path: target.path,
                        result,
                    });
                }) {
                Ok(worker) => self.aux_workers.push(worker),
                Err(error) => {
                    self.pane_mut(self.active_pane).error =
                        Some(format!("Could not inspect folder permissions: {error}"))
                }
            }
            return None;
        }
        if let Some(path) = self.focused_path(self.active_pane) {
            let mode = self
                .preview_state
                .content
                .as_ref()
                .filter(|preview| preview.path == path)
                .and_then(|preview| preview.metadata.mode)
                .unwrap_or(0o644);
            show_permissions_dialog(path, mode, sender);
        } else {
            self.pane_mut(self.active_pane).error =
                Some("Focus an item before editing permissions".to_owned());
        }
        None
    }

    pub(super) fn on_create_archive(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        if self.operation_sources(self.active_pane).is_empty() {
            self.pane_mut(self.active_pane).error =
                Some("Select one or more items to archive".to_owned());
        } else {
            show_create_archive_dialog(self.folder_action_input(sender));
        }
        None
    }

    pub(super) fn on_convert_image(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        if self
            .focused_item(self.active_pane)
            .is_some_and(|(path, kind)| supports_image_conversion(&path, kind))
        {
            show_image_conversion_dialog(sender);
        } else {
            self.pane_mut(self.active_pane).error =
                Some("Focus a supported image file to convert".to_owned());
        }
        None
    }

    pub(super) fn on_pdf_tools(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        let sources = self
            .operation_sources(self.active_pane)
            .into_iter()
            .filter(|path| {
                path.as_path()
                    .extension()
                    .and_then(OsStr::to_str)
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            })
            .collect::<Vec<_>>();
        if sources.is_empty() {
            self.pane_mut(self.active_pane).error = Some("Select one or more PDF files".to_owned());
        } else {
            show_pdf_tools_dialog(sources.len(), sender);
        }
        None
    }

    pub(super) fn on_select_inspector_page(&mut self, command: CommandId) -> Option<AppMsg> {
        self.preview_visible = true;
        self.refresh_grid_column_estimates();
        self.inspector_page = match command {
            CommandId::InspectorInfo => InspectorPage::Info,
            CommandId::InspectorWork => InspectorPage::Work,
            CommandId::InspectorLog => InspectorPage::Log,
            _ => unreachable!(),
        };
        self.focus_inspector_epoch = self.focus_inspector_epoch.wrapping_add(1);
        self.persist_session();
        None
    }

    pub(super) fn on_settings(&mut self, sender: &ComponentSender<Self>) -> Option<AppMsg> {
        settings::show(self, sender);
        None
    }

    pub(super) fn on_compare_directories(
        &mut self,
        sender: &ComponentSender<Self>,
    ) -> Option<AppMsg> {
        self.start_compare(sender);
        None
    }

    pub(super) fn execute_command(&mut self, command: CommandId, sender: &ComponentSender<Self>) {
        if command != CommandId::CommandPalette {
            self.palette_open = false;
            self.palette_query.clear();
            self.palette_selection = 0;
        }
        let decision = self.action_context(self.active_pane).action(command);
        if let Some(reason) = decision.reason {
            if decision.notify {
                self.pane_mut(self.active_pane).error = Some(reason.to_owned());
            }
            return;
        }
        let message = match command {
            CommandId::SwitchPane => Some(AppMsg::SwitchPane),
            CommandId::Open => Some(AppMsg::OpenCursor),
            CommandId::Parent => Some(AppMsg::UpActive),
            CommandId::Back => Some(AppMsg::BackActive),
            CommandId::Forward => Some(AppMsg::ForwardActive),
            CommandId::CursorUp => Some(AppMsg::MoveCursorVertical(-1, false)),
            CommandId::CursorDown => Some(AppMsg::MoveCursorVertical(1, false)),
            CommandId::CursorLeft => Some(AppMsg::MoveCursorHorizontal(-1, false)),
            CommandId::CursorRight => Some(AppMsg::MoveCursorHorizontal(1, false)),
            CommandId::CursorFirst => Some(AppMsg::MoveCursorTo(CursorTarget::First, false)),
            CommandId::CursorLast => Some(AppMsg::MoveCursorTo(CursorTarget::Last, false)),
            CommandId::CursorPageUp => Some(AppMsg::MoveCursorTo(CursorTarget::PageUp, false)),
            CommandId::CursorPageDown => Some(AppMsg::MoveCursorTo(CursorTarget::PageDown, false)),
            CommandId::ToggleSelection => Some(AppMsg::ToggleCursor),
            CommandId::SelectAll => Some(AppMsg::SelectAllActive),
            CommandId::ClearSelection => Some(AppMsg::ClearSelectionActive),
            CommandId::InvertSelection => Some(AppMsg::InvertSelectionActive),
            CommandId::ClearLayered => Some(AppMsg::ClearLayered),
            CommandId::NewTab => Some(AppMsg::NewTabActive),
            CommandId::CloseTab => Some(AppMsg::CloseTabActive),
            CommandId::PreviousTab | CommandId::NextTab => self.on_cycle_tab(command),
            CommandId::Refresh => Some(AppMsg::RefreshActive),
            CommandId::ToggleOrientation => Some(AppMsg::ToggleOrientation),
            CommandId::ToggleDualPane => Some(AppMsg::ToggleDualPane),
            CommandId::FocusFilter => {
                let pane = self.active_pane;
                self.pane_mut(pane).focus_filter_epoch =
                    self.pane(pane).focus_filter_epoch.wrapping_add(1);
                None
            }
            CommandId::FocusLocation => {
                self.focus_location_epoch = self.focus_location_epoch.wrapping_add(1);
                None
            }
            CommandId::Bookmark => self.on_bookmark(),
            CommandId::NewFavoriteGroup => {
                show_new_favorite_group_dialog(sender);
                None
            }
            CommandId::MatchOtherPane => {
                let path = self
                    .pane(self.active_pane.other())
                    .current_directory()
                    .clone();
                Some(AppMsg::Navigate(self.active_pane, path))
            }
            CommandId::SwapPanes => self.on_swap_panes(sender),
            CommandId::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                self.refresh_grid_column_estimates();
                self.persist_session();
                None
            }
            CommandId::FocusSidebar => self.on_focus_sidebar(),
            CommandId::TogglePreview => Some(AppMsg::TogglePreview),
            CommandId::FocusInspector => self.on_focus_inspector(),
            CommandId::QuickLook => Some(AppMsg::ToggleQuickLook),
            CommandId::ToggleHidden => Some(AppMsg::ToggleHiddenActive),
            CommandId::FocusFiles => {
                self.focus_active_files();
                None
            }
            CommandId::CommandPalette => self.on_command_palette(),
            CommandId::ClassicKeymap => {
                self.keymap.set_profile(KeymapProfile::Classic);
                self.persist_session();
                None
            }
            CommandId::ModernKeymap => {
                self.keymap.set_profile(KeymapProfile::Modern);
                self.persist_session();
                None
            }
            CommandId::DetailsView => Some(AppMsg::SetViewMode(PaneViewMode::List)),
            CommandId::CompactView => Some(AppMsg::SetViewMode(PaneViewMode::Columns)),
            CommandId::IconsView => Some(AppMsg::SetViewMode(PaneViewMode::Grid)),
            CommandId::SortByName => Some(AppMsg::SetSort(self.active_pane, SortKey::Name)),
            CommandId::SortBySize => Some(AppMsg::SetSort(self.active_pane, SortKey::Size)),
            CommandId::SortByModified => Some(AppMsg::SetSort(self.active_pane, SortKey::Modified)),
            CommandId::SortByType => Some(AppMsg::SetSort(self.active_pane, SortKey::Kind)),
            CommandId::ToggleSortDirection => Some(AppMsg::ToggleSortDirection(self.active_pane)),
            CommandId::SelectGlob => Some(AppMsg::OpenGlob(true)),
            CommandId::DeselectGlob => Some(AppMsg::OpenGlob(false)),
            CommandId::Copy | CommandId::Move | CommandId::Trash => {
                if command == CommandId::Trash
                    && self
                        .operation_sources(self.active_pane)
                        .iter()
                        .any(|path| self.is_archive_browse_path(path))
                {
                    self.review_archive_removal(sender);
                } else {
                    self.start_operation(command, sender);
                }
                None
            }
            CommandId::Rename => self.on_rename(sender),
            CommandId::BatchRename => self.on_batch_rename(sender),
            CommandId::OpenWith => {
                if let Some(path) = self.focused_path(self.active_pane) {
                    show_open_with_dialog(path);
                }
                None
            }
            CommandId::OpenInNewTab => self.on_open_in_new_tab(sender),
            CommandId::OpenOtherPane => self.on_open_other_pane(sender),
            CommandId::Reveal => {
                if let Some(path) = self.focused_path(self.active_pane) {
                    reveal_in_file_manager(path);
                }
                None
            }
            CommandId::CopyPath => self.on_copy_path(),
            CommandId::CopyDirectoryPath => self.on_copy_directory_path(),
            CommandId::CopyClipboard | CommandId::Cut => self.on_copy_or_cut(command),
            CommandId::Paste => {
                self.paste_file_clipboard(sender);
                None
            }
            CommandId::NewFile => {
                show_new_file_dialog(sender);
                None
            }
            CommandId::NewDirectory => {
                show_new_directory_dialog(sender);
                None
            }
            CommandId::DeletePermanent => self.on_delete_permanent(sender),
            CommandId::SecureDelete => {
                self.review_secure_delete(sender);
                None
            }
            CommandId::Undo => {
                self.start_history(HistoryDirection::Undo, sender);
                None
            }
            CommandId::Redo => {
                self.start_history(HistoryDirection::Redo, sender);
                None
            }
            CommandId::ViewFile | CommandId::EditFile => Some(AppMsg::OpenCursor),
            CommandId::RecursiveSearch => Some(AppMsg::OpenSearch(true)),
            CommandId::UnifiedSearch => Some(AppMsg::OpenSearch(false)),
            CommandId::Checksum => {
                self.start_checksum(sender);
                None
            }
            CommandId::Permissions => self.on_permissions(sender),
            CommandId::CreateArchive => self.on_create_archive(sender),
            CommandId::BrowseArchive => {
                if let Some((path, kind)) = self.focused_item(self.active_pane)
                    && kind != EntryKind::Directory
                    && is_archive_path(&path)
                {
                    self.start_archive_browse(self.active_pane, path, sender);
                } else {
                    self.pane_mut(self.active_pane).error =
                        Some("Select a ZIP, 7Z, TAR, TAR.GZ or TGZ archive to browse".to_owned());
                }
                None
            }
            CommandId::ExtractArchive => {
                self.start_extract_archive(sender);
                None
            }
            CommandId::ConvertImage => self.on_convert_image(sender),
            CommandId::PdfTools => self.on_pdf_tools(sender),
            CommandId::ToggleTerminal => Some(AppMsg::ToggleTerminal),
            CommandId::TogglePauseFirstOperation => Some(AppMsg::TogglePauseFirstOperation),
            CommandId::CancelFirstOperation => Some(AppMsg::CancelFirstOperation),
            CommandId::InspectorInfo | CommandId::InspectorWork | CommandId::InspectorLog => {
                self.on_select_inspector_page(command)
            }
            CommandId::SaveWorkspace => {
                show_save_workspace_dialog(sender);
                None
            }
            CommandId::ConnectRemote => {
                show_remote_dialog(
                    self.remote_uris.clone(),
                    self.remote_names.clone(),
                    None,
                    sender,
                );
                None
            }
            CommandId::Settings => self.on_settings(sender),
            CommandId::ShortcutReference => {
                show_shortcut_reference(&self.keymap);
                None
            }
            CommandId::TagRed => {
                self.set_focused_tag(Some("red"));
                None
            }
            CommandId::TagOrange => {
                self.set_focused_tag(Some("orange"));
                None
            }
            CommandId::TagYellow => {
                self.set_focused_tag(Some("yellow"));
                None
            }
            CommandId::TagGreen => {
                self.set_focused_tag(Some("green"));
                None
            }
            CommandId::TagBlue => {
                self.set_focused_tag(Some("blue"));
                None
            }
            CommandId::TagPurple => {
                self.set_focused_tag(Some("purple"));
                None
            }
            CommandId::ClearTag => {
                self.set_focused_tag(None);
                None
            }
            CommandId::FindDuplicates => {
                power_tools::duplicates(
                    Arc::clone(&self.vfs),
                    self.pane(self.active_pane).current_directory().clone(),
                    sender.input_sender().clone(),
                );
                None
            }
            CommandId::CompareFiles => {
                let selected = self.operation_sources(self.active_pane);
                let other = self
                    .focused_item(self.active_pane.other())
                    .map(|(path, _)| path);
                let pair = if selected.len() == 2 {
                    Some((selected[0].clone(), selected[1].clone()))
                } else {
                    selected.first().cloned().zip(other)
                };
                if let Some((left, right)) = pair {
                    power_tools::compare(Arc::clone(&self.vfs), left, right);
                } else {
                    self.pane_mut(self.active_pane).error =
                        Some("Select two files, or focus one file in each panel".into());
                }
                None
            }
            CommandId::CompareDirectories => self.on_compare_directories(sender),
        };
        if let Some(message) = message {
            let message = match &self.folder_action_target {
                Some(target) => target.message(message),
                None => message,
            };
            let _ = sender.input_sender().send(message);
        }
    }
}
