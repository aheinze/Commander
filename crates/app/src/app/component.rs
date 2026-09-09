//! The Relm4 `SimpleComponent` implementation: view construction and the message loop.

use super::*;

impl SimpleComponent for AppModel {
    type Init = AppInit;
    type Input = AppMsg;
    type Output = ();
    type Root = adw::ApplicationWindow;
    type Widgets = AppWidgets;

    fn init_root() -> Self::Root {
        adw::ApplicationWindow::builder()
            .application(&relm4::main_adw_application())
            .title(crate::APP_NAME)
            .icon_name("org.example.Dualpane")
            .default_width(1_520)
            .default_height(900)
            .build()
    }

    fn init(
        init: Self::Init,
        window: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        crate::icons::install(&gtk::prelude::WidgetExt::display(&window));
        let saved = init.session.unwrap_or_default();
        install_styles(saved.appearance);
        apply_color_theme(saved.color_theme, saved.appearance);
        window.set_default_size(saved.window_width.max(720), saved.window_height.max(480));
        let compact_layout = gtk::prelude::WidgetExt::display(&window)
            .monitors()
            .item(0)
            .and_downcast::<gdk::Monitor>()
            .is_some_and(|monitor| monitor.geometry().width() < 1_440);
        let home = home_path();
        let left_fallback = init.options.left.clone().unwrap_or_else(|| home.clone());
        let right_fallback = init.options.right.clone().unwrap_or_else(|| home.clone());
        let left_session = settings::startup_pane(
            &saved.left,
            saved.workflow.restore_tabs,
            init.options.left.as_ref(),
        );
        let right_session = settings::startup_pane(
            &saved.right,
            saved.workflow.restore_tabs,
            init.options.right.as_ref(),
        );
        let vfs: Arc<dyn Vfs> = Arc::new(LocalFs);
        let operation_engine = OperationEngine::new(Arc::clone(&vfs))
            .with_journal_directory(recovery::journal_directory());
        let (thumbnail_scheduler, thumbnail_bridge) =
            match ThumbnailScheduler::new(Arc::clone(&vfs)) {
                Ok(scheduler) => {
                    let results = scheduler.results();
                    let input = sender.input_sender().clone();
                    match thread::Builder::new()
                        .name("dualpane-thumbnail-bridge".to_owned())
                        .spawn(move || {
                            while let Ok(response) = results.recv() {
                                if input.send(AppMsg::ThumbnailReady(response)).is_err() {
                                    break;
                                }
                            }
                        }) {
                        Ok(bridge) => (Some(scheduler), Some(bridge)),
                        Err(error) => {
                            tracing::warn!(%error, "could not start thumbnail result bridge");
                            (None, None)
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "could not start thumbnail workers");
                    (None, None)
                }
            };
        let mut model = AppModel {
            folder_action_target: None,
            devices: devices::DeviceState::new(),
            panes: [
                PaneState::from_session(&left_session, left_fallback),
                PaneState::from_session(&right_session, right_fallback),
            ],
            active_pane: PaneId::Left,
            vertical_split: saved.vertical_split,
            dual_pane: saved.dual_pane,
            split_position: saved.split_position.max(120),
            keymap: Keymap::new(saved.keymap_profile, init.keymap_overrides),
            sidebar_visible: saved.sidebar_visible,
            collapsed_sidebar_groups: saved.collapsed_sidebar_groups.clone(),
            preview_visible: saved.preview_visible && !compact_layout,
            preview_width: saved
                .preview_width
                .clamp(PREVIEW_MIN_WIDTH, PREVIEW_MAX_WIDTH),
            quick_look_open: false,
            inspector_page: InspectorPage::Info,
            preview_state: PreviewState::new(),
            inspector_selection: InspectorSelectionState::new(),
            folder_measure: FolderMeasureState::new(),
            inspector_git: InspectorGitState::new(),
            bookmarks: saved
                .bookmarks
                .iter()
                .map(|path| VPath::from(path.as_str()))
                .collect(),
            bookmark_labels: saved.bookmark_labels.clone(),
            favorite_groups: saved.favorite_groups.clone(),
            recent: saved
                .recent
                .iter()
                .map(|path| VPath::from(path.as_str()))
                .collect(),
            window_width: saved.window_width.max(720),
            window_height: saved.window_height.max(480),
            workspaces: saved.workspaces.clone(),
            remote_uris: saved
                .remote_uris
                .iter()
                .filter_map(|uri| {
                    remote::RemoteConnection::parse(uri)
                        .ok()
                        .map(|connection| connection.uri)
                })
                .collect(),
            remote_names: saved
                .remote_names
                .iter()
                .filter(|(uri, name)| saved.remote_uris.contains(uri) && !name.trim().is_empty())
                .filter_map(|(uri, name)| {
                    remote::RemoteConnection::parse(uri)
                        .ok()
                        .map(|connection| (connection.uri, name.trim().to_owned()))
                })
                .collect(),
            appearance: saved.appearance,
            color_theme: saved.color_theme,
            parallel_transfers: saved.parallel_transfers,
            workflow: saved.workflow.clone(),
            custom_tools: saved.custom_tools.clone(),
            custom_tools_revision: 0,
            tags: saved.tags.clone(),
            tags_revision: 0,
            palette_open: false,
            palette_query: String::new(),
            palette_selection: 0,
            focus_location_epoch: 0,
            focus_sidebar_epoch: 0,
            focus_inspector_epoch: 0,
            vfs,
            operation_engine,
            active_operations: 0,
            operations: BTreeMap::new(),
            operation_log: VecDeque::new(),
            operation_log_revision: 0,
            next_operation_log_id: 1,
            pending_history: BTreeMap::new(),
            undo_stack: init.history.undo,
            redo_stack: init.history.redo,
            history_busy: false,
            recovery_records: Vec::new(),
            recovery_errors: init.history_warning.into_iter().collect(),
            clipboard_cut_jobs: BTreeMap::new(),
            clipboard_provider: None,
            clipboard_generation: 0,
            search_open: false,
            search_content_preset: false,
            search_generation: 0,
            search_cancel: None,
            search_loading: false,
            search_results: SearchResults::default(),
            search_error: None,
            search_session: None,
            tool_cancel: None,
            archive_mounts: archive_browser::ArchiveLocations::default(),
            terminal_visible: false,
            terminal_tabs: Vec::new(),
            active_terminal: None,
            next_terminal_id: 1,
            terminal_revision: 0,
            session_worker: init.session_worker,
            started: init.started,
            profile_startup: init.options.profile_startup,
            benchmark_mode: init.options.benchmark_mode,
            benchmark_filter: init.options.benchmark_filter,
            startup_reported: false,
            filter_benchmark_started: false,
            filter_benchmark_scheduled: false,
            filter_benchmark_started_at: [None, None],
            filter_benchmark_results: [None, None],
            filter_benchmark_worker_results: [None, None],
            quit_after_first_paint: init.options.quit_after_first_paint,
            first_paints: [None, None],
            complete_paints: [None, None],
            scroll_results: [None, None],
            scroll_epoch: 0,
            rss_pending: false,
            thumbnail_scheduler,
            thumbnail_bridge,
            thumbnail_request_id: 0,
            thumbnail_outstanding: HashMap::new(),
            thumbnail_applied: HashMap::new(),
            thumbnail_revision: 0,
            thumbnail_event: None,
            aux_workers: Vec::new(),
            live_updates: model_watch::LiveUpdates::default(),
        };
        for pane in &mut model.panes {
            pane.sort.directories_first = model.workflow.directories_first;
        }
        if !model.workflow.remember_recent {
            model.recent.clear();
        }
        model.workflow.recent_limit = model.workflow.recent_limit.clamp(5, 100);
        model.recent.truncate(model.workflow.recent_limit as usize);
        model.panes[PaneId::Left.index()].focus_files_epoch = 1;
        model.refresh_grid_column_estimates();

        let app_shell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        app_shell.add_css_class("app-shell");
        app_shell.set_hexpand(true);
        app_shell.set_vexpand(true);
        let main_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main_box.add_css_class("app-main");
        main_box.set_hexpand(true);
        main_box.set_vexpand(true);

        let topbar = topbar::TopBarWidgets::new(&window, &sender);
        let global_search = topbar.search.clone();
        main_box.append(&topbar.root);

        let palette_dialog = adw::Dialog::builder()
            .content_width(560)
            .content_height(430)
            .build();
        palette_dialog.add_css_class("command-palette-dialog");
        let palette_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        palette_box.add_css_class("command-palette");
        let palette_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        palette_header.add_css_class("command-palette-header");
        let palette_entry = gtk::SearchEntry::new();
        palette_entry.update_property(&[gtk::accessible::Property::Label(
            "Search available commands",
        )]);
        palette_entry.add_css_class("command-palette-search");
        palette_entry.set_hexpand(true);
        palette_entry.set_placeholder_text(Some("Search commands…"));
        palette_header.append(&palette_entry);
        palette_header.append(&chrome::dialog_close_button(&palette_dialog));
        palette_box.append(&palette_header);
        let palette_scroll = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        palette_scroll.add_css_class("command-palette-scroll");
        let palette_commands = gtk::Box::new(gtk::Orientation::Vertical, 3);
        palette_commands.add_css_class("command-palette-results");
        palette_scroll.set_child(Some(&palette_commands));
        palette_box.append(&palette_scroll);
        let palette_footer = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        palette_footer.add_css_class("command-palette-footer");
        for (keys, text) in [
            (&["↑", "↓"][..], "Navigate"),
            (&["↵"][..], "Run"),
            (&["Esc"][..], "Close"),
        ] {
            palette_footer.append(&palette_hint(keys, text));
        }
        palette_box.append(&palette_footer);
        palette_dialog.set_child(Some(&palette_box));
        {
            let input = sender.input_sender().clone();
            palette_entry.connect_search_changed(move |entry| {
                let _ = input.send(AppMsg::SetPaletteQuery(entry.text().to_string()));
            });
        }
        {
            let input = sender.input_sender().clone();
            palette_entry.connect_activate(move |_| {
                let _ = input.send(AppMsg::ActivatePaletteSelection);
            });
        }
        let palette_keys = gtk::EventControllerKey::new();
        palette_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let input = sender.input_sender().clone();
            let entry = palette_entry.clone();
            palette_keys.connect_key_pressed(move |_, key, _, modifiers| {
                if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
                    let _ = input.send(AppMsg::SetPaletteQuery(entry.text().to_string()));
                    let _ = input.send(AppMsg::ActivatePaletteSelection);
                    return glib::Propagation::Stop;
                }
                let message = match key {
                    gdk::Key::Escape => Some(AppMsg::ClosePalette),
                    gdk::Key::BackSpace => {
                        Some(AppMsg::SetPaletteQuery(palette_delete_backward(&entry)))
                    }
                    gdk::Key::Up | gdk::Key::KP_Up => Some(AppMsg::MovePaletteSelection(-1)),
                    gdk::Key::Down | gdk::Key::KP_Down => Some(AppMsg::MovePaletteSelection(1)),
                    gdk::Key::Page_Up | gdk::Key::KP_Page_Up => {
                        Some(AppMsg::MovePaletteSelection(-7))
                    }
                    gdk::Key::Page_Down | gdk::Key::KP_Page_Down => {
                        Some(AppMsg::MovePaletteSelection(7))
                    }
                    gdk::Key::Home | gdk::Key::KP_Home => {
                        Some(AppMsg::MovePaletteSelectionToEnd(false))
                    }
                    gdk::Key::End | gdk::Key::KP_End => {
                        Some(AppMsg::MovePaletteSelectionToEnd(true))
                    }
                    _ if !modifiers.intersects(
                        gdk::ModifierType::CONTROL_MASK
                            | gdk::ModifierType::ALT_MASK
                            | gdk::ModifierType::SUPER_MASK
                            | gdk::ModifierType::META_MASK,
                    ) =>
                    {
                        key.to_unicode().and_then(|character| {
                            (!character.is_control()).then(|| {
                                AppMsg::SetPaletteQuery(palette_insert_character(&entry, character))
                            })
                        })
                    }
                    _ => None,
                };
                if let Some(message) = message {
                    let _ = input.send(message);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
        }
        palette_dialog.add_controller(palette_keys);
        let palette_presented = Rc::new(Cell::new(false));
        let palette_input_active = Rc::new(Cell::new(false));
        let palette_pending_open = Rc::new(Cell::new(false));
        {
            let input = sender.input_sender().clone();
            let palette_presented = Rc::clone(&palette_presented);
            let palette_input_active = Rc::clone(&palette_input_active);
            let palette_pending_open = Rc::clone(&palette_pending_open);
            palette_dialog.connect_closed(move |_| {
                palette_presented.set(false);
                palette_input_active.set(false);
                palette_pending_open.set(false);
                let _ = input.send(AppMsg::ClosePalette);
            });
        }

        let tag_store = Rc::new(RefCell::new(model.tags.clone()));
        let custom_tool_store = Rc::new(RefCell::new(model.custom_tools.clone()));
        let thumbnail_ui = Rc::new(RefCell::new(ThumbnailUiState::new()));
        let file_drag_ui = Rc::new(FileDragUiState::default());
        let panes = [
            PaneWidgets::new(
                PaneId::Left,
                &sender,
                model.keymap.clone(),
                Rc::clone(&tag_store),
                Rc::clone(&custom_tool_store),
                Rc::clone(&thumbnail_ui),
                Rc::clone(&file_drag_ui),
            ),
            PaneWidgets::new(
                PaneId::Right,
                &sender,
                model.keymap.clone(),
                Rc::clone(&tag_store),
                Rc::clone(&custom_tool_store),
                Rc::clone(&thumbnail_ui),
                Rc::clone(&file_drag_ui),
            ),
        ];
        let paned = gtk::Paned::new(if model.vertical_split {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        });
        paned.set_widget_name("file-pane-split");
        paned.set_wide_handle(true);
        paned.set_resize_start_child(true);
        paned.set_resize_end_child(true);
        paned.set_shrink_start_child(true);
        paned.set_shrink_end_child(true);
        paned.set_position(model.split_position);
        paned.set_start_child(Some(&panes[0].root));
        paned.set_end_child(Some(&panes[1].root));
        paned.set_hexpand(true);
        paned.set_vexpand(true);
        let SidebarWidgets {
            revealer: sidebar_revealer,
            focus_target: sidebar_focus_target,
            groups: sidebar_groups,
            bookmarks: sidebar_bookmarks,
            recent: sidebar_recent,
            workspaces: sidebar_workspaces,
            remotes: sidebar_remotes,
            devices: sidebar_devices,
            places: sidebar_places,
        } = build_sidebar(
            &window,
            &sender,
            Rc::clone(&file_drag_ui),
            &model.collapsed_sidebar_groups,
        );
        let preview = PreviewWidgets::new(&sender);
        let quick_look = QuickLookWidgets::new(&window, &sender);
        let search = SearchWidgets::new(&window, &sender, model.keymap.clone());
        let workspace_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        workspace_paned.set_widget_name("inspector-split");
        workspace_paned.add_css_class("workspace");
        workspace_paned.set_wide_handle(false);
        workspace_paned.set_resize_start_child(true);
        workspace_paned.set_resize_end_child(false);
        workspace_paned.set_shrink_start_child(false);
        workspace_paned.set_shrink_end_child(true);
        workspace_paned.set_start_child(Some(&paned));
        workspace_paned.set_end_child(Some(&preview.root));
        workspace_paned.set_hexpand(true);
        workspace_paned.set_vexpand(true);
        let terminal = TerminalWidgets::new(&sender, &workspace_paned);
        main_box.append(&terminal.root);
        let jobs = job_view::JobActivityWidgets::new(sender.input_sender());
        main_box.append(&jobs.root);
        app_shell.append(&sidebar_revealer);
        app_shell.append(&main_box);
        window.set_content(Some(&notifications::wrap(&app_shell)));

        let inner_panes = paned.clone();
        let initial_preview_visible = model.preview_visible;
        let initial_preview_width = model.preview_width;
        workspace_paned.connect_map(move |workspace| {
            let workspace = workspace.clone();
            let inner_panes = inner_panes.clone();
            glib::idle_add_local_once(move || {
                if initial_preview_visible && workspace.width() > 700 {
                    workspace.set_position(workspace.width().saturating_sub(initial_preview_width));
                } else {
                    workspace.set_position(workspace.width());
                }
                glib::timeout_add_local_once(Duration::from_millis(240), move || {
                    let extent = if inner_panes.orientation() == gtk::Orientation::Horizontal {
                        if initial_preview_visible {
                            workspace.position()
                        } else {
                            workspace.width()
                        }
                    } else {
                        inner_panes.height()
                    };
                    if extent > 0 {
                        inner_panes.set_position(extent / 2);
                    }
                });
            });
        });
        {
            let input = sender.input_sender().clone();
            let latest = Rc::new(Cell::new(model.preview_width));
            let pending = Rc::new(Cell::new(false));
            workspace_paned.connect_position_notify(move |paned| {
                let width = paned.width().saturating_sub(paned.position());
                if width < PREVIEW_MIN_WIDTH {
                    return;
                }
                latest.set(width);
                if pending.replace(true) {
                    return;
                }
                let input = input.clone();
                let latest = Rc::clone(&latest);
                let pending = Rc::clone(&pending);
                glib::timeout_add_local_once(Duration::from_millis(120), move || {
                    pending.set(false);
                    let _ = input.send(AppMsg::PreviewWidth(latest.get()));
                });
            });
        }
        {
            let reset_click = gtk::GestureClick::new();
            reset_click.set_button(gdk::BUTTON_PRIMARY);
            let workspace = workspace_paned.clone();
            let input = sender.input_sender().clone();
            reset_click.connect_pressed(move |_, presses, x, _| {
                if presses == 2 && (x - f64::from(workspace.position())).abs() <= 10.0 {
                    let _ = input.send(AppMsg::ResetPreviewWidth);
                }
            });
            workspace_paned.add_controller(reset_click);
        }
        layout::install_pane_constraints(&paned);

        {
            let input = sender.input_sender().clone();
            let latest = Rc::new(Cell::new(model.split_position));
            let pending = Rc::new(Cell::new(false));
            paned.connect_position_notify(move |paned| {
                latest.set(paned.position());
                if pending.replace(true) {
                    return;
                }
                let input = input.clone();
                let latest = Rc::clone(&latest);
                let pending = Rc::clone(&pending);
                glib::timeout_add_local_once(Duration::from_millis(120), move || {
                    pending.set(false);
                    let _ = input.send(AppMsg::SplitPosition(latest.get()));
                });
            });
        }
        {
            let latest = Rc::new(Cell::new((model.window_width, model.window_height)));
            let last_sent = Rc::new(Cell::new((0, 0)));
            let pending = Rc::new(Cell::new(false));
            let connected = Rc::new(Cell::new(false));
            let input = sender.input_sender().clone();
            window.connect_realize(move |window| {
                if connected.replace(true) {
                    return;
                }
                let Some(surface) = window.surface() else {
                    connected.set(false);
                    return;
                };
                let input = input.clone();
                let latest = Rc::clone(&latest);
                let last_sent = Rc::clone(&last_sent);
                let pending = Rc::clone(&pending);
                surface.connect_layout(move |_, width, height| {
                    latest.set((width, height));
                    if pending.replace(true) {
                        return;
                    }
                    let input = input.clone();
                    let latest = Rc::clone(&latest);
                    let last_sent = Rc::clone(&last_sent);
                    let pending = Rc::clone(&pending);
                    glib::timeout_add_local_once(Duration::from_millis(250), move || {
                        pending.set(false);
                        let (width, height) = latest.get();
                        if last_sent.replace((width, height)) != (width, height) {
                            let _ = input.send(AppMsg::WindowSize(width, height));
                        }
                    });
                });
            });
        }
        install_shortcuts(
            &window,
            &sender,
            model.keymap.clone(),
            PaletteKeyboardState {
                input_active: Rc::clone(&palette_input_active),
                pending_open: Rc::clone(&palette_pending_open),
                presented: Rc::clone(&palette_presented),
                entry: palette_entry.clone(),
                dialog: palette_dialog.clone(),
            },
        );
        let omarchy_theme_monitor = install_omarchy_theme_monitor(&sender);

        let widgets = AppWidgets {
            paned,
            workspace_paned,
            sidebar_revealer,
            sidebar_focus_target,
            sidebar_groups,
            sidebar_favorite_groups: Vec::new(),
            sidebar_bookmarks,
            sidebar_recent,
            sidebar_workspaces,
            sidebar_remotes,
            sidebar_devices,
            sidebar_places,
            sidebar_bookmark_rows: Vec::new(),
            sidebar_recent_rows: Vec::new(),
            sidebar_remote_rows: Vec::new(),
            rendered_active_location: None,
            rendered_sidebar_visible: !model.sidebar_visible,
            rendered_focus_sidebar_epoch: 0,
            rendered_focus_inspector_epoch: 0,
            rendered_preview_visible: !model.preview_visible,
            rendered_preview_width: model.preview_width,
            rendered_bookmarks: None,
            rendered_recent: vec!["\0".to_owned()],
            rendered_workspaces: vec!["\0".to_owned()],
            rendered_remotes: vec!["\0".to_owned()],
            rendered_remote_names: BTreeMap::new(),
            rendered_remote_devices: None,
            palette_dialog,
            palette_parent: window.clone(),
            palette_entry,
            palette_scroll,
            palette_commands,
            palette_presented,
            palette_input_active,
            palette_pending_open,
            rendered_palette: None,
            topbar,
            global_search,
            rendered_dual_pane: !model.dual_pane,
            rendered_focus_filter_epoch: 0,
            rendered_focus_location_epoch: 0,
            jobs,
            preview,
            quick_look,
            search,
            terminal,
            tag_store,
            custom_tool_store,
            rendered_tags_revision: u64::MAX,
            rendered_custom_tools_revision: u64::MAX,
            thumbnail_ui,
            rendered_thumbnail_revision: u64::MAX,
            file_drag_ui,
            panes,
            rendered_scroll_epoch: 0,
            _omarchy_theme_monitor: omarchy_theme_monitor,
        };

        model.start_listing(PaneId::Left, &sender);
        model.start_listing(PaneId::Right, &sender);
        pane_git::install_refresh(&window, &sender);
        model.sync_pane_git(&sender);
        model.start_preview(&sender);
        model.persist_session();
        model.scan_recovery(false, &sender);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>) {
        self.reap_aux_workers();
        match message {
            AppMsg::RefreshPaneGit => {
                for pane in &mut self.panes {
                    pane.git.invalidate();
                }
            }
            AppMsg::PaneGitReady { pane, path, info } => {
                self.on_pane_git_ready(pane, path, info);
            }
            AppMsg::ShowRecovery => self.scan_recovery(true, &sender),
            AppMsg::RecoveryReady {
                records,
                errors,
                show,
            } => {
                let interrupted = records
                    .iter()
                    .any(|record| record.state.is_none() && !record.reviewed);
                self.recovery_records = records;
                self.recovery_errors.extend(errors);
                if show || interrupted {
                    self.show_recovery(&sender);
                } else if !self.recovery_errors.is_empty() {
                    notifications::error(&self.recovery_errors.join("\n"));
                }
            }
            AppMsg::RestoreArchive(path) => self.restore_archive(path, &sender),
            AppMsg::ArchiveRestoreReady { source, result } => {
                self.on_archive_restore_ready(source, result, &sender)
            }
            AppMsg::RestoreMissingOriginals(path) => self.restore_missing_originals(path, &sender),
            AppMsg::ReviewRecovery(path) => self.review_recovery(path, &sender),
            AppMsg::RecoveryFinished(result) => {
                match &result {
                    Ok(message) => notifications::success(message),
                    Err(error) => notifications::error(error),
                }
                let message = result.unwrap_or_else(|error| error);
                self.push_operation_log(message.clone());
                self.start_listing(PaneId::Left, &sender);
                self.start_listing(PaneId::Right, &sender);
            }
            AppMsg::ExecuteCommand(command) => self.execute_command(command, &sender),
            AppMsg::PermissionsInspected { pane, path, result } => match result {
                Ok(mode) => show_permissions_dialog(path, mode, &sender),
                Err(error) => self.pane_mut(pane).error = Some(error),
            },
            AppMsg::TabFolderAction { target, action } => {
                self.on_tab_folder_action(target, *action, &sender);
            }
            AppMsg::DevicesChanged => self.devices.refresh(),
            AppMsg::DeviceMountRemoved(root) => {
                self.devices.refresh();
                self.leave_device_paths(&[root], &sender);
            }
            AppMsg::OpenDevice(key) => self.start_device_action(&key, false, &sender),
            AppMsg::RemoveDevice(key) => self.start_device_action(&key, true, &sender),
            AppMsg::DeviceFinished(result) => self.finish_device_action(result, &sender),
            AppMsg::NavigateActive(path) => self.navigate(self.active_pane, path, &sender),
            AppMsg::SetPaletteQuery(query) => {
                if self.palette_query != query {
                    self.palette_query = query;
                    self.palette_selection = 0;
                }
            }
            AppMsg::ClosePalette => {
                self.palette_open = false;
                self.palette_query.clear();
                self.palette_selection = 0;
                self.focus_active_files();
            }
            AppMsg::OmarchyThemeChanged => {
                apply_color_theme(self.color_theme, self.appearance);
            }
            AppMsg::MovePaletteSelection(delta) => self.on_move_palette_selection(delta),
            AppMsg::MovePaletteSelectionToEnd(end) => {
                let count = self.palette_items().len();
                self.palette_selection = if end { count.saturating_sub(1) } else { 0 };
            }
            AppMsg::ActivatePaletteSelection => {
                self.activate_palette_selection(&sender);
            }
            AppMsg::ActivatePaletteItem(index) => {
                self.palette_selection = index;
                self.activate_palette_selection(&sender);
            }
            AppMsg::OpenGlob(select) => self.on_open_glob(select),
            AppMsg::SetGlob(pane, query) => self.pane_mut(pane).glob_query = query,
            AppMsg::ApplyGlob(pane) => self.start_glob(pane, &sender),
            AppMsg::ActivatePane(pane) => {
                self.active_pane = pane;
                // Pointer presses already focus their target. Refocusing the
                // file view here jumps from an ancestor to the last Miller column
                // before release, and can scroll the clicked row away.
                self.ensure_filter_worker(pane, &sender);
                self.start_preview(&sender);
            }
            AppMsg::SwitchPane => {
                self.active_pane = self.active_pane.other();
                self.focus_active_files();
                self.ensure_filter_worker(self.active_pane, &sender);
                self.start_preview(&sender);
            }
            AppMsg::BackActive => {
                let _ = sender.input_sender().send(AppMsg::Back(self.active_pane));
            }
            AppMsg::ForwardActive => {
                let _ = sender
                    .input_sender()
                    .send(AppMsg::Forward(self.active_pane));
            }
            AppMsg::UpActive => {
                let _ = sender.input_sender().send(AppMsg::Up(self.active_pane));
            }
            AppMsg::RefreshActive => {
                let _ = sender
                    .input_sender()
                    .send(AppMsg::Refresh(self.active_pane));
            }
            AppMsg::NewTabActive => {
                let _ = sender.input_sender().send(AppMsg::NewTab(self.active_pane));
            }
            AppMsg::CloseTabActive => {
                let _ = sender
                    .input_sender()
                    .send(AppMsg::CloseTab(self.active_pane));
            }
            AppMsg::ToggleOrientation => {
                self.vertical_split = !self.vertical_split;
                self.refresh_grid_column_estimates();
                self.persist_session();
            }
            AppMsg::ToggleDualPane => {
                self.dual_pane = !self.dual_pane;
                self.refresh_grid_column_estimates();
                self.focus_active_files();
                self.persist_session();
            }
            AppMsg::SetPaneFilter(pane, query) => self.set_filter(pane, query, &sender),
            AppMsg::SetViewMode(mode) => self.on_set_view_mode(mode, &sender),
            AppMsg::SetSort(pane, key) => self.on_set_sort(pane, key, &sender),
            AppMsg::ToggleSortDirection(pane) => self.on_toggle_sort_direction(pane, &sender),
            AppMsg::ToggleHiddenActive => self.on_toggle_hidden_active(&sender),
            AppMsg::TogglePreview => self.on_toggle_preview(&sender),
            AppMsg::ToggleQuickLook => {
                self.quick_look_open = !self.quick_look_open;
                if self.quick_look_open {
                    self.start_preview(&sender);
                }
            }
            AppMsg::OpenSearch(content) => {
                self.search_open = true;
                self.search_content_preset = content;
            }
            AppMsg::SearchAction(request) => self.search_action(request, false, &sender),
            AppMsg::SearchDeleteConfirmed(request) => self.search_action(request, true, &sender),
            AppMsg::RefreshSearch => self.refresh_search(&sender),
            AppMsg::CloseSearch => self.on_close_search(),
            AppMsg::CancelSearch => self.on_cancel_search(),
            AppMsg::RunSearch(options) => self.start_recursive_search(options, &sender),
            AppMsg::SearchReady { generation, result } => self.on_search_ready(generation, result),
            AppMsg::SyncReady(result) => self.on_sync_ready(result, &sender),
            AppMsg::ChecksumReady { path, result } => {
                self.tool_cancel = None;
                show_checksum_result(&path, result);
            }
            AppMsg::ChecksumsReady {
                left,
                right,
                result,
            } => self.on_checksums_ready(left, right, result),
            AppMsg::ApplyPermissions {
                path,
                mode,
                recursive,
            } => self.start_permissions(path, mode, recursive, &sender),
            AppMsg::PermissionsReady {
                path,
                mode,
                recursive,
                result,
            } => self.on_permissions_ready(path, mode, recursive, result, &sender),
            AppMsg::ApplyElevatedPermissions {
                path,
                mode,
                recursive,
            } => self.start_elevated_permissions(path, mode, recursive, &sender),
            AppMsg::ElevatedPermissionsReady(result) => {
                self.on_elevated_permissions_ready(result, &sender)
            }
            AppMsg::CreateArchive {
                name,
                format,
                password,
            } => {
                self.start_create_archive(name, format, password, &sender);
            }
            AppMsg::ArchivePasswordRequested(request) => self.on_archive_password_request(request),
            AppMsg::ArchiveProgress { id, progress } => self.on_archive_progress(id, progress),
            AppMsg::ArchiveReady { id, pane, result } => {
                self.on_archive_ready(id, pane, result, &sender)
            }
            AppMsg::ArchiveBrowseReady { pane, id, result } => {
                self.on_archive_browse_ready(pane, id, result, &sender)
            }
            AppMsg::ArchiveReloadReady { source, id, result } => {
                self.on_archive_reload_ready(source, id, result, &sender)
            }
            AppMsg::ConvertImage(format) => self.start_image_conversion(format, &sender),
            AppMsg::ImageConverted(result) => self.on_image_converted(result, &sender),
            AppMsg::RunPdfTool {
                options,
                other_pane,
            } => self.start_pdf_tool(options, other_pane, &sender),
            AppMsg::PdfReady(result) => self.on_pdf_ready(result, &sender),
            AppMsg::ToggleTerminal => {
                self.terminal_visible = !self.terminal_visible;
                if self.terminal_visible && self.terminal_tabs.is_empty() {
                    self.start_terminal(&sender);
                }
            }
            AppMsg::NewTerminal => {
                self.terminal_visible = true;
                self.start_terminal(&sender);
            }
            AppMsg::SelectTerminal(id) => self.on_select_terminal(id),
            AppMsg::CloseTerminal(id) => self.close_terminal(id),
            AppMsg::CloseAllTerminals => self.on_close_all_terminals(),
            AppMsg::TerminalStarted { id, result } => self.on_terminal_started(id, result, &sender),
            AppMsg::TerminalEvent(id, event) => self.on_terminal_event(id, event),
            AppMsg::TerminalInput(bytes) => self.on_terminal_input(bytes),
            AppMsg::ResizeTerminal { id, rows, cols } => self.on_resize_terminal(id, rows, cols),
            AppMsg::SetInspectorPage(page) => self.inspector_page = page,
            AppMsg::ClearOperationLog => {
                self.operation_log.clear();
                self.operation_log_revision = self.operation_log_revision.wrapping_add(1);
            }
            AppMsg::DismissOperation(id) => self.on_dismiss_operation(id),
            AppMsg::RetryOperation(id) => self.retry_operation(id, &sender),
            AppMsg::CreateDirectory(name) => self.create_directory(name, &sender),
            AppMsg::DirectoryCreated { pane, result } => match result {
                Ok(()) => self.start_listing(pane, &sender),
                Err(error) => self.pane_mut(pane).error = Some(error),
            },
            AppMsg::CreateFile(name) => self.create_file(name, &sender),
            AppMsg::FileCreated { pane, result } => match result {
                Ok(()) => self.start_listing(pane, &sender),
                Err(error) => self.pane_mut(pane).error = Some(error),
            },
            AppMsg::RenamePath(source, name) => self.rename_path(source, name, &sender),
            AppMsg::RenameFinished {
                pane,
                source,
                destination,
                result,
            } => self.on_rename_finished(pane, source, destination, result, &sender),
            AppMsg::UpdateArchive { pane, request } => {
                self.start_archive_update(pane, request, &sender)
            }
            AppMsg::ArchiveUpdateReady {
                id,
                pane,
                source,
                result,
            } => self.on_archive_update_ready(id, pane, source, result, &sender),
            AppMsg::BatchRename(items) => self.start_batch_rename(items, &sender),
            AppMsg::BatchRenameFinished(result) => self.on_batch_rename_finished(result, &sender),
            AppMsg::DeletePermanentConfirmed => {
                self.start_operation(CommandId::DeletePermanent, &sender);
            }
            AppMsg::SecureDeleteConfirmed { pane, plan } => {
                self.start_secure_delete(pane, plan, &sender)
            }
            AppMsg::SecureDeleteProgress { id, progress } => {
                self.on_secure_delete_progress(id, progress)
            }
            AppMsg::SecureDeleteReady { id, pane, result } => {
                self.on_secure_delete_ready(id, pane, result, &sender)
            }
            AppMsg::OpenFailed(pane, error) => self.pane_mut(pane).error = Some(error),
            AppMsg::LoadPreview { generation, path } => {
                self.begin_preview_load(generation, path, &sender);
            }
            AppMsg::PreviewReady {
                generation,
                path,
                result,
            } => self.on_preview_ready(generation, path, result, &sender),
            AppMsg::SelectionSummaryReady {
                generation,
                summary,
            } => self.on_selection_summary_ready(generation, summary, &sender),
            AppMsg::FolderMeasureReady { generation, result } => {
                self.on_folder_measure_ready(generation, result)
            }
            AppMsg::GitInfoReady { generation, info } => {
                if generation == self.inspector_git.generation {
                    self.inspector_git.loading = false;
                    self.inspector_git.info = info;
                }
            }
            AppMsg::RequestThumbnail {
                pane,
                generation,
                path,
            } => self.on_request_thumbnail(pane, generation, path),
            AppMsg::ThumbnailReady(response) => self.on_thumbnail_ready(response),
            AppMsg::AppendFilter(character) => {
                self.typeahead(character);
                self.start_preview(&sender);
            }
            AppMsg::ClearLayered => self.clear_layered(&sender),
            AppMsg::SelectionChanged(pane, selection, cursor_row) => {
                self.on_selection_changed(pane, selection, cursor_row, &sender)
            }
            AppMsg::ContextTarget(pane, target) => {
                self.focus_context_target(pane, target);
                self.start_preview(&sender);
            }
            AppMsg::PasteInto(pane, destination) => {
                self.paste_file_clipboard_into(pane, destination, &sender);
            }
            AppMsg::MoveCursor(delta, extend) => self.on_move_cursor(delta, extend, &sender),
            AppMsg::MoveCursorVertical(delta, extend) => {
                self.move_cursor_vertical(self.active_pane, delta, extend);
                self.start_preview(&sender);
            }
            AppMsg::MoveCursorHorizontal(delta, extend) => {
                self.move_cursor_horizontal(self.active_pane, delta, extend, &sender);
                self.start_preview(&sender);
            }
            AppMsg::MoveCursorTo(target, extend) => {
                self.move_cursor_to(self.active_pane, target, extend);
                self.start_preview(&sender);
            }
            AppMsg::ToggleCursor => {
                self.toggle_cursor(self.active_pane);
                self.start_preview(&sender);
            }
            AppMsg::SelectAllActive => {
                self.select_all(self.active_pane);
                self.start_preview(&sender);
            }
            AppMsg::ClearSelectionActive => {
                self.clear_selection(self.active_pane);
                self.start_preview(&sender);
            }
            AppMsg::InvertSelectionActive => {
                self.invert_selection(self.active_pane);
                self.start_preview(&sender);
            }
            AppMsg::OpenCursor => self.on_open_cursor(&sender),
            AppMsg::Navigate(pane, path) => self.navigate(pane, path, &sender),
            AppMsg::NavigateExact(pane, path) => self.navigate_exact(pane, path, &sender),
            AppMsg::CancelLocation(pane) => {
                self.active_pane = pane;
                self.focus_active_files();
            }
            AppMsg::Back(pane) => self.on_back(pane, &sender),
            AppMsg::Forward(pane) => self.on_forward(pane, &sender),
            AppMsg::Up(pane) => {
                // In column view, "up" first folds the deepest column back, the same way
                // Left does, so the browsing context is not thrown away in one step.
                if self.pane(pane).view_mode == PaneViewMode::Columns
                    && self.pane(pane).miller_columns.len() > 1
                {
                    self.move_miller_left(pane, &sender);
                } else if let Some(parent) =
                    self.archive_mounts.parent(&self.pane(pane).active().path)
                {
                    self.navigate_exact(pane, parent, &sender);
                }
            }
            AppMsg::Refresh(pane) => {
                let path = self.pane(pane).current_directory();
                if self.archive_mounts.contains(path) {
                    let target = self.archive_mounts.display(path);
                    self.start_archive_location(pane, target, true, &sender);
                } else {
                    self.start_listing(pane, &sender);
                }
            }
            AppMsg::OpenRow(pane, row) => self.open_row(pane, row, &sender),
            AppMsg::MillerOpen(pane, column, row) => {
                self.open_miller_row(pane, column, row, &sender);
            }
            AppMsg::MillerSelectionChanged {
                pane,
                column,
                path,
                selection,
                row,
            } => {
                if self
                    .pane_mut(pane)
                    .select_miller_rows(column, &path, selection, row)
                {
                    self.active_pane = pane;
                    self.start_preview(&sender);
                }
            }
            AppMsg::ClipboardFiles {
                pane,
                destination,
                result,
                owner,
            } => match result {
                Ok((sources, cut)) => {
                    self.start_clipboard_transfer(pane, sources, cut, destination, owner, &sender)
                }
                Err(error) => self.pane_mut(pane).error = Some(error),
            },
            AppMsg::NavigationScroll {
                pane,
                path,
                x,
                value,
            } => {
                let state = self.pane_mut(pane);
                if x && state.active().path == path {
                    state.miller_scroll_x = value;
                } else if !x && state.view_mode == PaneViewMode::Columns {
                    if let Some(column) = state
                        .miller_columns
                        .iter_mut()
                        .find(|column| column.path == path)
                    {
                        column.scroll_y = value;
                    }
                } else if !x && state.active().path == path {
                    state.scroll_y = value;
                }
                self.persist_session();
            }
            AppMsg::MillerResize(pane, path, width) => {
                if let Some(column) = self
                    .pane_mut(pane)
                    .miller_columns
                    .iter_mut()
                    .find(|column| column.path == path)
                {
                    column.width = width.clamp(220, 600);
                    self.pane_mut(pane).miller_revision =
                        self.pane(pane).miller_revision.wrapping_add(1);
                }
                self.persist_session();
            }
            AppMsg::MillerRetry(pane, column, path) => {
                if self
                    .pane(pane)
                    .miller_columns
                    .get(column)
                    .is_some_and(|current| current.path == path)
                {
                    if column == 0 {
                        self.start_listing(pane, &sender);
                    } else if let Some(row) =
                        self.pane(pane).miller_columns[column - 1].selected_row
                    {
                        self.open_miller_row(pane, column - 1, row, &sender);
                    }
                }
            }
            AppMsg::MillerActivateFile(pane, path, kind) => {
                if !kind.is_directory() {
                    self.open_path(pane, kind, path, &sender);
                }
            }
            AppMsg::MillerRefreshed {
                pane,
                generation,
                snapshots,
            } => self.on_miller_refreshed(pane, generation, snapshots, &sender),
            AppMsg::NewTab(pane) => self.on_new_tab(pane, &sender),
            AppMsg::CloseTab(pane) => {
                let tab = self.pane(pane).active_tab;
                self.close_tab(pane, tab, &sender);
            }
            AppMsg::CloseTabAt(pane, tab) => self.close_tab(pane, tab, &sender),
            AppMsg::SelectTab(pane, tab) => self.on_select_tab(pane, tab, &sender),
            AppMsg::SplitPosition(position) => {
                if position > 0 && position != self.split_position {
                    self.split_position = position;
                    self.persist_session();
                }
            }
            AppMsg::PreviewWidth(width) => self.on_preview_width(width),
            AppMsg::ResetPreviewWidth => {
                if self.preview_width != PREVIEW_DEFAULT_WIDTH {
                    self.preview_width = PREVIEW_DEFAULT_WIDTH;
                    self.persist_session();
                }
            }
            AppMsg::WindowSize(width, height) => self.on_window_size(width, height),
            AppMsg::SaveWorkspace(name) => self.on_save_workspace(name),
            AppMsg::SidebarLocation { path, action } => {
                self.sidebar_location(path, action, &sender)
            }
            AppMsg::RemoveRecent(path) => {
                self.recent.retain(|saved| saved != &path);
                self.persist_session();
            }
            AppMsg::ClearRecent => {
                self.recent.clear();
                self.persist_session();
            }
            AppMsg::UpdateWorkspace(index) => self.on_update_workspace(index),
            AppMsg::RenameWorkspace { index, name } => self.on_rename_workspace(index, name),
            AppMsg::RemoveWorkspace(index) => {
                if index < self.workspaces.len() {
                    self.workspaces.remove(index);
                    self.persist_session();
                }
            }
            AppMsg::OpenWorkspace(index) => self.on_open_workspace(index, &sender),
            AppMsg::MoveBookmark { from, before } => self.on_move_bookmark(from, before),
            AppMsg::MoveBookmarkToEnd(from) => self.on_move_bookmark_to_end(from),
            AppMsg::RemoveBookmark(index) => {
                if index < self.bookmarks.len() {
                    let path = self.bookmarks.remove(index);
                    self.bookmark_labels.remove(&path.to_string());
                    self.persist_session();
                }
            }
            AppMsg::RenameFavorite { group, path, name } => {
                self.on_rename_favorite(group, &path, &name)
            }
            AppMsg::SetSidebarGroupExpanded { key, expanded } => {
                let changed = if expanded {
                    self.collapsed_sidebar_groups.remove(&key)
                } else {
                    self.collapsed_sidebar_groups.insert(key)
                };
                if changed {
                    self.persist_session();
                }
            }
            AppMsg::CreateFavoriteGroup(name) => self.on_create_favorite_group(name),
            AppMsg::RemoveFavoriteGroup(index) => {
                if index < self.favorite_groups.len() {
                    let group = self.favorite_groups.remove(index);
                    self.collapsed_sidebar_groups
                        .remove(&sidebar::favorite_group_key(&group.name));
                    self.persist_session();
                }
            }
            AppMsg::AddCurrentToFavoriteGroup(index) => {
                self.on_add_current_to_favorite_group(index)
            }
            AppMsg::RemoveGroupedFavorite { group, item } => {
                self.on_remove_grouped_favorite(group, item)
            }
            AppMsg::ConnectRemote(uri) => self.on_connect_remote(uri, &sender),
            AppMsg::RemoveRemote(uri) => {
                self.remote_uris.retain(|saved| saved != &uri);
                self.remote_names.remove(&uri);
                self.persist_session();
            }
            AppMsg::SaveRemote {
                uri,
                replacing,
                name,
            } => {
                self.save_remote(&uri, Some(&replacing), Some(&name));
            }
            AppMsg::RemoteConnected { uri, result } => {
                self.on_remote_connected(uri, result, &sender)
            }
            AppMsg::EditRemote(uri) => {
                show_remote_dialog(
                    self.remote_uris.clone(),
                    self.remote_names.clone(),
                    Some(uri),
                    &sender,
                );
            }
            AppMsg::ConnectRemoteWithOptions {
                connection,
                replacing,
                name,
            } => {
                self.on_connect_remote_with_options(connection, replacing, name, &sender);
            }
            AppMsg::ForgetRemotePassword(uri) => {
                let input = sender.input_sender().clone();
                let worker = thread::Builder::new()
                    .name("dualpane-forget-password".to_owned())
                    .spawn(move || {
                        let result = forget_remote_password(&uri);
                        let _ = input.send(AppMsg::RemotePasswordForgotten { uri, result });
                    });
                match worker {
                    Ok(worker) => self.aux_workers.push(worker),
                    Err(error) => self.push_operation_log(format!(
                        "Could not forget the saved password: {error}"
                    )),
                }
            }
            AppMsg::RemotePasswordForgotten { uri, result } => match result {
                Ok(_) => {
                    self.push_operation_log(format!("Forgot the saved password for {uri}"));
                    self.on_connect_remote(uri, &sender);
                }
                Err(error) => self.pane_mut(self.active_pane).error = Some(error),
            },
            AppMsg::SetSettings(settings) => self.on_set_settings(*settings, &sender),
            AppMsg::SettingsSaveFailed(error) => {
                notifications::error(&format!("Could not save keyboard shortcuts: {error}"));
            }
            AppMsg::ManageCustomTools => {
                dialogs::show_custom_tools_dialog(self.custom_tools.clone(), &sender);
            }
            AppMsg::SetCustomTools(tools) => {
                self.custom_tools = tools;
                self.custom_tools_revision = self.custom_tools_revision.wrapping_add(1);
                self.persist_session();
            }
            AppMsg::RunCustomTool(index) => self.run_custom_tool(index, &sender),
            AppMsg::CustomToolFinished(result) => match result {
                Ok(message) => self.push_operation_log(message),
                Err(error) => self.pane_mut(self.active_pane).error = Some(error),
            },
            AppMsg::DropFiles {
                sources,
                destination,
                action,
            } => self.start_drop_transfer(sources, destination, action, &sender),
            AppMsg::Listing {
                pane,
                generation,
                event,
            } => self.handle_listing(pane, generation, event, &sender),
            AppMsg::MetadataVisible(pane, source_index) => {
                self.request_metadata(pane, source_index, &sender);
            }
            AppMsg::MeasureSelection { pane, generation } => {
                self.measure_selection(pane, generation, &sender);
            }
            AppMsg::SelectionSizeReady {
                pane,
                generation,
                result,
            } => {
                self.pane_mut(pane)
                    .finish_selection_size(generation, result);
            }
            AppMsg::FilesystemChanged {
                path,
                watch_id,
                batch,
            } => {
                self.on_filesystem_changed(path, watch_id, batch, &sender);
            }
            AppMsg::FilesystemUpdated(result) => self.on_filesystem_updated(result, &sender),
            AppMsg::MetadataReady {
                pane,
                generation,
                result,
            } => self.handle_metadata(pane, generation, result, &sender),
            AppMsg::FilterReady {
                pane,
                generation,
                queue_delay,
                result,
            } => self.handle_filter(pane, generation, queue_delay, result),
            AppMsg::FilterWorkerReady(pane) => {
                self.pane_mut(pane).filter_ready = true;
                self.maybe_start_filter_benchmark(&sender);
            }
            AppMsg::StartFilterBenchmark => self.on_start_filter_benchmark(&sender),
            AppMsg::GlobReady {
                pane,
                generation,
                select,
                keys,
                elapsed,
            } => self.on_glob_ready(pane, generation, select, keys, elapsed),
            AppMsg::FramePainted {
                pane,
                complete,
                elapsed,
            } => self.handle_frame_painted(pane, complete, elapsed, &sender),
            AppMsg::ScrollFinished(pane, metrics) => {
                self.on_scroll_finished(pane, metrics, &sender)
            }
            AppMsg::RssMeasured(rss_kib) => {
                self.report_benchmark(rss_kib);
                relm4::main_application().quit();
            }
            AppMsg::OperationEvent { kind, event } => self.on_operation_event(kind, event, &sender),
            AppMsg::ResolveConflict {
                job_id,
                conflict_id,
                choice,
                apply_to_all,
            } => self.on_resolve_conflict(job_id, conflict_id, choice, apply_to_all),
            AppMsg::CompareConflictChecksum {
                pane,
                job_id,
                conflict_id,
                source,
                destination,
            } => self.on_compare_conflict_checksum(
                pane,
                job_id,
                conflict_id,
                source,
                destination,
                &sender,
            ),
            AppMsg::CancelFirstOperation => {
                if let Some(id) = job_view::first_active_operation(&self.operations) {
                    let _ = sender.input_sender().send(AppMsg::CancelOperation(id));
                }
            }
            AppMsg::TogglePauseFirstOperation => self.on_toggle_pause_first_operation(),
            AppMsg::CancelOperation(id) => {
                if let Some(operation) = self.operations.get(&id)
                    && operation.can_control()
                {
                    operation.control.cancel();
                }
            }
            AppMsg::TogglePauseOperation(id) => self.on_toggle_pause_operation(id),
            AppMsg::OperationFinished(finished) => self.on_operation_finished(finished, &sender),
            AppMsg::HistoryFinished {
                entry,
                direction,
                result,
            } => self.on_history_finished(entry, direction, result, &sender),
        }
        self.sync_directory_watches(&sender);
        self.flush_filesystem_changes(&sender);
        self.sync_pane_git(&sender);
        for pane in [PaneId::Left, PaneId::Right] {
            self.sync_selection_size(pane, &sender);
        }
    }

    fn update_view(&self, widgets: &mut Self::Widgets, sender: ComponentSender<Self>) {
        for pane in [PaneId::Left, PaneId::Right] {
            let state = self.pane(pane);
            notifications::observe(
                format!("pane-{pane:?}"),
                state.error.as_deref(),
                notifications::Kind::Error,
            );
            for (index, column) in state.miller_columns.iter().enumerate() {
                notifications::observe(
                    format!("column-{pane:?}-{index}"),
                    column.error.as_deref(),
                    notifications::Kind::Error,
                );
            }
        }
        for (id, operation) in &self.operations {
            notifications::observe(
                format!("operation-{id:?}"),
                operation.error.as_deref(),
                if operation.state == JobState::Cancelled {
                    notifications::Kind::Info
                } else {
                    notifications::Kind::Error
                },
            );
        }
        let reserved_width = i32::from(self.sidebar_visible) * SIDEBAR_WIDTH
            + i32::from(self.preview_visible) * self.preview_width;
        let available_pane_width = self.window_width.saturating_sub(reserved_width);
        let responsive_vertical = available_pane_width < 840;
        let orientation = if self.vertical_split || responsive_vertical {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        };
        widgets.file_drag_ui.history_busy.set(self.history_busy);
        widgets.topbar.render(self);
        if widgets.paned.orientation() != orientation {
            widgets.paned.set_orientation(orientation);
        }
        let active_index = self.active_pane.index();
        widgets.panes[0]
            .root
            .set_visible(self.dual_pane || active_index == 0);
        widgets.panes[1]
            .root
            .set_visible(self.dual_pane || active_index == 1);
        if widgets.rendered_dual_pane != self.dual_pane {
            widgets.rendered_dual_pane = self.dual_pane;
            if self.dual_pane {
                let paned = widgets.paned.clone();
                glib::idle_add_local_once(move || {
                    let extent = if paned.orientation() == gtk::Orientation::Horizontal {
                        paned.width()
                    } else {
                        paned.height()
                    };
                    if extent > 0 {
                        paned.set_position(extent / 2);
                    }
                });
            }
        }
        if widgets.rendered_sidebar_visible != self.sidebar_visible {
            widgets.rendered_sidebar_visible = self.sidebar_visible;
            widgets.sidebar_revealer.set_visible(self.sidebar_visible);
            widgets
                .sidebar_revealer
                .set_reveal_child(self.sidebar_visible);
            let paned = widgets.paned.clone();
            glib::timeout_add_local_once(Duration::from_millis(280), move || {
                let extent = if paned.orientation() == gtk::Orientation::Horizontal {
                    paned.width()
                } else {
                    paned.height()
                };
                if extent > 0 {
                    paned.set_position(extent / 2);
                }
            });
        }
        if widgets.rendered_preview_visible != self.preview_visible {
            widgets.rendered_preview_visible = self.preview_visible;
            widgets.preview.root.set_visible(self.preview_visible);
            widgets.preview.root.set_reveal_child(self.preview_visible);
            if self.preview_visible {
                let workspace = widgets.workspace_paned.clone();
                let panes = widgets.paned.clone();
                let preview_width = self.preview_width;
                let list_adjustments = [
                    widgets.panes[0].list_hadjustment.clone(),
                    widgets.panes[1].list_hadjustment.clone(),
                ];
                glib::timeout_add_local_once(Duration::from_millis(220), move || {
                    if workspace.width() > 700 {
                        workspace.set_position(workspace.width().saturating_sub(preview_width));
                    }
                    let extent = if panes.orientation() == gtk::Orientation::Horizontal {
                        workspace.position()
                    } else {
                        panes.height()
                    };
                    if extent > 0 {
                        panes.set_position(extent / 2);
                    }
                    for adjustment in list_adjustments {
                        adjustment.set_value(adjustment.lower());
                    }
                });
            } else {
                let workspace = widgets.workspace_paned.clone();
                let panes = widgets.paned.clone();
                glib::timeout_add_local_once(Duration::from_millis(220), move || {
                    workspace.set_position(workspace.width());
                    let extent = if panes.orientation() == gtk::Orientation::Horizontal {
                        workspace.width()
                    } else {
                        panes.height()
                    };
                    if extent > 0 {
                        panes.set_position(extent / 2);
                    }
                });
            }
        }
        if self.preview_visible && widgets.rendered_preview_width != self.preview_width {
            widgets.rendered_preview_width = self.preview_width;
            let workspace = &widgets.workspace_paned;
            if workspace.width() > 700 {
                workspace.set_position(workspace.width().saturating_sub(self.preview_width));
            }
        }
        widgets.render_sidebar_bookmarks(
            &self.bookmarks,
            &self.bookmark_labels,
            &self.favorite_groups,
            &self.collapsed_sidebar_groups,
            &sender,
        );
        widgets.render_sidebar_recent(&self.recent, &sender);
        widgets.render_sidebar_workspaces(&self.workspaces, &sender);
        widgets.render_sidebar_remotes(
            &self.remote_uris,
            &self.remote_names,
            &self.devices,
            &sender,
        );
        if widgets.sidebar_devices.render(
            &self.devices,
            &self.remote_uris,
            &sender,
            &widgets.file_drag_ui,
        ) {
            widgets.rendered_active_location = None;
        }
        widgets.render_sidebar_active(self.pane(self.active_pane).current_directory());
        for group in widgets
            .sidebar_groups
            .iter()
            .chain(&widgets.sidebar_favorite_groups)
        {
            group.set_expanded(!self.collapsed_sidebar_groups.contains(&group.key));
        }
        if widgets.rendered_focus_sidebar_epoch != self.focus_sidebar_epoch {
            widgets.rendered_focus_sidebar_epoch = self.focus_sidebar_epoch;
            let target = widgets.sidebar_focus_target.clone();
            glib::idle_add_local_once(move || {
                target.grab_focus();
            });
        }
        widgets.render_palette(self, &sender);

        let active_state = self.pane(self.active_pane);
        if widgets.rendered_focus_filter_epoch != active_state.focus_filter_epoch {
            widgets.rendered_focus_filter_epoch = active_state.focus_filter_epoch;
            widgets.global_search.grab_focus();
        }
        if widgets.rendered_focus_location_epoch != self.focus_location_epoch {
            widgets.rendered_focus_location_epoch = self.focus_location_epoch;
            widgets.panes[self.active_pane.index()].focus_location();
        }
        if widgets.rendered_tags_revision != self.tags_revision {
            widgets.rendered_tags_revision = self.tags_revision;
            widgets.tag_store.borrow_mut().clone_from(&self.tags);
        }
        if widgets.rendered_custom_tools_revision != self.custom_tools_revision {
            widgets.rendered_custom_tools_revision = self.custom_tools_revision;
            widgets
                .custom_tool_store
                .borrow_mut()
                .clone_from(&self.custom_tools);
        }
        if widgets.rendered_thumbnail_revision != self.thumbnail_revision {
            widgets.rendered_thumbnail_revision = self.thumbnail_revision;
            if let Some(event) = &self.thumbnail_event {
                widgets
                    .thumbnail_ui
                    .borrow_mut()
                    .complete(&event.path, event.thumbnail.as_ref());
            }
        }
        for pane in [PaneId::Left, PaneId::Right] {
            widgets.panes[pane.index()].pane_drag.borrow_mut().actions = self.action_context(pane);
        }
        widgets.panes[0].render(
            &self.panes[0],
            self.active_pane == PaneId::Left,
            self.started,
            self.tags_revision,
            &self.archive_mounts,
            &sender,
        );
        widgets.panes[1].render(
            &self.panes[1],
            self.active_pane == PaneId::Right,
            self.started,
            self.tags_revision,
            &self.archive_mounts,
            &sender,
        );
        widgets.preview.render(self, &sender);
        widgets.jobs.render(&self.operations, sender.input_sender());
        if widgets.rendered_focus_inspector_epoch != self.focus_inspector_epoch {
            widgets.rendered_focus_inspector_epoch = self.focus_inspector_epoch;
            widgets.preview.focus_page(self.inspector_page);
        }
        widgets.quick_look.render(self);
        widgets.search.render(self, &sender);
        widgets.terminal.render(self, &sender);

        if self.scroll_epoch > widgets.rendered_scroll_epoch {
            widgets.rendered_scroll_epoch = self.scroll_epoch;
            match self.scroll_epoch {
                1 => widgets.panes[0].start_scroll_benchmark(&sender),
                2 => widgets.panes[1].start_scroll_benchmark(&sender),
                _ => {}
            }
        }
    }
}

/// A footer hint made of one or more key caps followed by a short description.
fn palette_hint(keys: &[&str], text: &str) -> gtk::Box {
    let hint = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hint.add_css_class("command-palette-hint");
    for key in keys {
        let cap = gtk::Label::new(Some(key));
        cap.add_css_class("command-key");
        hint.append(&cap);
    }
    hint.append(&gtk::Label::new(Some(text)));
    hint
}
