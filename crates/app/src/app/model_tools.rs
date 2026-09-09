//! Long-running tools: search, compare/sync, checksums, permissions, archives, PDF, and terminals.

use super::*;

impl AppModel {
    pub(super) fn start_recursive_search(
        &mut self,
        options: SearchOptions,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(cancel) = self.search_cancel.take() {
            cancel.cancel();
        }
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        self.search_loading = true;
        self.search_error = None;
        self.search_results = SearchResults::default();
        let root = self.pane(self.active_pane).current_directory().clone();
        let cancel = CancelToken::new();
        self.search_cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-search".to_owned())
            .spawn(move || {
                let result = recursive_search(vfs.as_ref(), &root, &options, &cancel);
                let _ = input.send(AppMsg::SearchReady { generation, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.search_loading = false;
                self.search_cancel = None;
                self.search_error = Some(format!("Could not start search: {error}"));
            }
        }
    }

    pub(super) fn start_compare(&mut self, sender: &ComponentSender<Self>) {
        let _ = compare_view::show(
            Arc::clone(&self.vfs),
            self.operation_engine.clone(),
            self.pane(PaneId::Left).current_directory().clone(),
            self.pane(PaneId::Right).current_directory().clone(),
            sender.input_sender().clone(),
        );
    }

    pub(super) fn start_checksum(&mut self, sender: &ComponentSender<Self>) {
        let paths = self.operation_sources(self.active_pane);
        let Some(path) = paths.first().cloned() else {
            self.pane_mut(self.active_pane).error =
                Some("Focus a file before computing its checksum".to_owned());
            return;
        };
        if paths.len() > 2 {
            self.pane_mut(self.active_pane).error =
                Some("Select one file to calculate or two files to compare".to_owned());
            return;
        }
        if let Some(cancel) = self.tool_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancelToken::new();
        self.tool_cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        let comparison = paths.get(1).cloned();
        match thread::Builder::new()
            .name("dualpane-checksum".to_owned())
            .spawn(move || {
                if let Some(right) = comparison {
                    let result = sha256(vfs.as_ref(), &path, &cancel).and_then(|left_hash| {
                        sha256(vfs.as_ref(), &right, &cancel)
                            .map(|right_hash| (left_hash, right_hash))
                    });
                    let _ = input.send(AppMsg::ChecksumsReady {
                        left: path,
                        right,
                        result,
                    });
                } else {
                    let result = sha256(vfs.as_ref(), &path, &cancel);
                    let _ = input.send(AppMsg::ChecksumReady { path, result });
                }
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.tool_cancel = None;
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not start checksum: {error}"));
            }
        }
    }

    pub(super) fn start_permissions(
        &mut self,
        path: VPath,
        mode: u32,
        recursive: bool,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(cancel) = self.tool_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancelToken::new();
        self.tool_cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        let worker_path = path.clone();
        match thread::Builder::new()
            .name("dualpane-permissions".to_owned())
            .spawn(move || {
                let result = set_mode_tree(vfs.as_ref(), worker_path, mode, recursive, &cancel);
                let _ = input.send(AppMsg::PermissionsReady {
                    path,
                    mode,
                    recursive,
                    result,
                });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.tool_cancel = None;
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not start permissions update: {error}"));
            }
        }
    }

    pub(super) fn start_elevated_permissions(
        &mut self,
        path: VPath,
        mode: u32,
        recursive: bool,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(cancel) = self.tool_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancelToken::new();
        self.tool_cancel = Some(cancel.clone());
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-elevated-permissions".to_owned())
            .spawn(move || {
                let result =
                    dualpane_platform::set_mode_elevated(path.as_path(), mode, recursive, &cancel)
                        .map_err(|error| error.to_string());
                let _ = input.send(AppMsg::ElevatedPermissionsReady(result));
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.tool_cancel = None;
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not start administrator retry: {error}"));
            }
        }
    }

    pub(super) fn start_create_archive(
        &mut self,
        name: String,
        format: ArchiveFormat,
        password: Option<crate::archive::Password>,
        sender: &ComponentSender<Self>,
    ) {
        let name = name.trim();
        if !valid_file_name(name) {
            self.pane_mut(self.active_pane).error = Some("Enter a valid archive name".to_owned());
            return;
        }
        let sources = self.operation_sources(self.active_pane);
        if sources.is_empty() {
            return;
        }
        let suffix = format!(".{}", format.extension());
        let file_name = if name.to_ascii_lowercase().ends_with(&suffix) {
            name.to_owned()
        } else {
            format!("{name}{suffix}")
        };
        let Some(parent) = sources[0].parent() else {
            self.pane_mut(self.active_pane).error =
                Some("Cannot archive the filesystem root".to_owned());
            return;
        };
        let destination = parent.join_name(OsStr::new(&file_name));
        self.start_archive_operation(
            OperationRetry::Archive {
                pane: self.active_pane,
                sources,
                destination,
                format: Some(format),
                password,
            },
            sender,
        );
    }

    pub(super) fn start_extract_archive(&mut self, sender: &ComponentSender<Self>) {
        let Some(source) = self.focused_path(self.active_pane) else {
            self.pane_mut(self.active_pane).error = Some("Focus an archive to extract".to_owned());
            return;
        };
        let destination = self
            .pane(self.active_pane.other())
            .current_directory()
            .clone();
        self.start_archive_operation(
            OperationRetry::Archive {
                pane: self.active_pane,
                sources: vec![source],
                destination,
                format: None,
                password: None,
            },
            sender,
        );
    }

    pub(super) fn start_image_conversion(
        &mut self,
        format: ImageOutputFormat,
        sender: &ComponentSender<Self>,
    ) {
        let Some((source, kind)) = self.focused_item(self.active_pane) else {
            return;
        };
        if !supports_image_conversion(&source, kind) {
            self.pane_mut(self.active_pane).error =
                Some("Focus a supported image file to convert".to_owned());
            return;
        }
        let Some(parent) = source.parent() else {
            self.pane_mut(self.active_pane).error = Some("Cannot convert this path".to_owned());
            return;
        };
        let mut name = source
            .as_path()
            .file_stem()
            .map_or_else(|| OsString::from("converted"), OsString::from);
        name.push("-converted.");
        name.push(format.extension());
        let destination = parent.join_name(&name);
        if let Some(cancel) = self.tool_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancelToken::new();
        self.tool_cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-image-convert".to_owned())
            .spawn(move || {
                let result = convert_image(vfs.as_ref(), &source, &destination, format, &cancel)
                    .map(|()| destination);
                let _ = input.send(AppMsg::ImageConverted(result));
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.tool_cancel = None;
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not start image conversion: {error}"));
            }
        }
    }

    pub(super) fn start_pdf_tool(
        &mut self,
        options: PdfToolOptions,
        other_pane: bool,
        sender: &ComponentSender<Self>,
    ) {
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
            self.pane_mut(self.active_pane).error = Some("Select one or more PDFs".to_owned());
            return;
        }
        let destination = if other_pane {
            self.pane(self.active_pane.other())
                .current_directory()
                .clone()
        } else {
            self.pane(self.active_pane).current_directory().clone()
        };
        if let Some(cancel) = self.tool_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancelToken::new();
        self.tool_cancel = Some(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-pdf-tool".to_owned())
            .spawn(move || {
                let result = run_pdf_tool(vfs.as_ref(), &sources, &destination, &options, &cancel);
                let _ = input.send(AppMsg::PdfReady(result));
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.tool_cancel = None;
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not start PDF tool: {error}"));
            }
        }
    }

    pub(super) fn start_terminal(&mut self, sender: &ComponentSender<Self>) {
        let cwd = self.pane(self.active_pane).current_directory().clone();
        self.start_terminal_at(cwd, sender);
    }

    pub(super) fn start_terminal_at(&mut self, cwd: VPath, sender: &ComponentSender<Self>) {
        let id = self.next_terminal_id;
        self.next_terminal_id = self.next_terminal_id.wrapping_add(1).max(1);
        self.terminal_tabs.push(TerminalTabState {
            id,
            title: terminal_title(&cwd),
            cwd: cwd.to_string(),
            starting: true,
            exited: false,
            session: None,
            screen: Rc::new(RefCell::new(TerminalScreenState::new())),
        });
        self.active_terminal = Some(id);
        self.terminal_revision = self.terminal_revision.wrapping_add(1);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name(format!("dualpane-terminal-{id}-start"))
            .spawn(move || {
                let result = TerminalSession::spawn(cwd.as_path());
                let _ = input.send(AppMsg::TerminalStarted { id, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                if let Some(tab) = self.terminal_tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.starting = false;
                    tab.exited = true;
                    tab.screen
                        .borrow_mut()
                        .process(format!("Could not start terminal: {error}\r\n").as_bytes());
                }
                self.terminal_revision = self.terminal_revision.wrapping_add(1);
            }
        }
    }

    pub(super) fn close_terminal(&mut self, id: u64) {
        let Some(index) = self.terminal_tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let closing_active = self.active_terminal == Some(id);
        self.terminal_tabs.remove(index);
        if closing_active {
            self.active_terminal = replacement_terminal_index(self.terminal_tabs.len(), index)
                .map(|replacement| self.terminal_tabs[replacement].id);
        }
        self.terminal_revision = self.terminal_revision.wrapping_add(1);
    }

    pub(super) fn attach_terminal_session(
        &mut self,
        id: u64,
        session: TerminalSession,
        events: std::sync::mpsc::Receiver<TerminalEvent>,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tab) = self.terminal_tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        tab.starting = false;
        let (rows, cols) = tab.screen.borrow().parser.screen().size();
        session.resize(rows, cols);
        tab.session = Some(session);
        self.terminal_revision = self.terminal_revision.wrapping_add(1);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name(format!("dualpane-terminal-{id}-bridge"))
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    let exited = matches!(event, TerminalEvent::Exited);
                    if input.send(AppMsg::TerminalEvent(id, event)).is_err() || exited {
                        break;
                    }
                }
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                if let Some(tab) = self.terminal_tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.session.take();
                    tab.exited = true;
                    tab.screen
                        .borrow_mut()
                        .process(format!("Could not monitor terminal: {error}\r\n").as_bytes());
                }
                self.terminal_revision = self.terminal_revision.wrapping_add(1);
            }
        }
    }

    pub(super) fn run_custom_tool(&mut self, index: usize, sender: &ComponentSender<Self>) {
        if let Some(reason) = self.action_context(self.active_pane).custom_tool_reason() {
            self.pane_mut(self.active_pane).error = Some(reason.into());
            return;
        }
        let Some(tool) = self
            .custom_tools
            .get(index)
            .cloned()
            .filter(|tool| tool.enabled)
        else {
            self.pane_mut(self.active_pane).error = Some("Custom tool is unavailable".to_owned());
            return;
        };
        let Some((focused, kind)) = self.focused_item(self.active_pane) else {
            self.pane_mut(self.active_pane).error =
                Some("Focus an item before running a custom tool".to_owned());
            return;
        };
        if (tool.applies_to == "files" && kind != EntryKind::File)
            || (tool.applies_to == "folders" && kind != EntryKind::Directory)
        {
            self.pane_mut(self.active_pane).error =
                Some(format!("{} does not apply to this item type", tool.name));
            return;
        }
        if !tool.extensions.trim().is_empty() && kind == EntryKind::File {
            let extension = focused
                .as_path()
                .extension()
                .and_then(OsStr::to_str)
                .unwrap_or_default();
            let allowed = tool
                .extensions
                .split([',', ' ', ';'])
                .filter(|value| !value.is_empty())
                .any(|value| extension.eq_ignore_ascii_case(value.trim_start_matches('.')));
            if !allowed {
                self.pane_mut(self.active_pane).error = Some(format!(
                    "{} does not apply to .{extension} files",
                    tool.name
                ));
                return;
            }
        }
        let paths = self.operation_sources(self.active_pane);
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("dualpane-custom-tool".to_owned())
            .spawn(move || {
                let result = launch_custom_tool(&tool, &paths);
                let _ = input.send(AppMsg::CustomToolFinished(result));
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.pane_mut(self.active_pane).error =
                    Some(format!("Could not start custom tool: {error}"));
            }
        }
    }

    pub(super) fn on_close_search(&mut self) {
        self.search_open = false;
        self.on_cancel_search();
    }

    pub(super) fn on_cancel_search(&mut self) {
        // Late completions must not replace a closed or newly opened search.
        self.search_generation = self.search_generation.wrapping_add(1);
        if let Some(cancel) = self.search_cancel.take() {
            cancel.cancel();
            self.search_error = Some("Search stopped".to_owned());
        }
        self.search_loading = false;
    }

    pub(super) fn on_search_ready(
        &mut self,
        generation: u64,
        result: Result<SearchResults, String>,
    ) {
        if generation == self.search_generation {
            self.search_loading = false;
            self.search_cancel = None;
            match result {
                Ok(results) => {
                    self.search_results = results;
                    self.search_error = None;
                }
                Err(error) => {
                    self.search_results = SearchResults::default();
                    self.search_error = Some(error);
                }
            }
        }
    }

    pub(super) fn on_sync_ready(
        &mut self,
        result: Result<usize, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(count) => {
                self.push_operation_log(format!("Folder synchronization applied {count} change(s)"))
            }
            Err(error) => self.pane_mut(self.active_pane).error = Some(error),
        }
        self.start_listing(PaneId::Left, sender);
        self.start_listing(PaneId::Right, sender);
    }

    pub(super) fn on_checksums_ready(
        &mut self,
        left: VPath,
        right: VPath,
        result: Result<(String, String), String>,
    ) {
        self.tool_cancel = None;
        show_checksum_comparison(&left, &right, result);
    }

    pub(super) fn on_permissions_ready(
        &mut self,
        path: VPath,
        mode: u32,
        recursive: bool,
        result: Result<usize, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.tool_cancel = None;
        match result {
            Ok(count) => {
                self.push_operation_log(format!("Updated permissions on {count} item(s)"));
                self.start_listing(PaneId::Left, sender);
                self.start_listing(PaneId::Right, sender);
            }
            Err(error) => {
                let permission_error = error.to_ascii_lowercase().contains("permission")
                    || error
                        .to_ascii_lowercase()
                        .contains("operation not permitted");
                if permission_error {
                    show_elevated_permissions_dialog(path, mode, recursive, &error, sender);
                } else {
                    self.pane_mut(self.active_pane).error = Some(error);
                }
            }
        }
    }

    pub(super) fn on_elevated_permissions_ready(
        &mut self,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        self.tool_cancel = None;
        match result {
            Ok(()) => {
                self.push_operation_log(
                    "Updated permissions with administrator authorization".to_owned(),
                );
                self.start_listing(PaneId::Left, sender);
                self.start_listing(PaneId::Right, sender);
            }
            Err(error) => self.pane_mut(self.active_pane).error = Some(error),
        }
    }

    pub(super) fn on_image_converted(
        &mut self,
        result: Result<VPath, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.tool_cancel = None;
        match result {
            Ok(path) => {
                self.push_operation_log(format!("Created converted image {path}"));
                self.start_listing(self.active_pane, sender);
            }
            Err(error) => self.pane_mut(self.active_pane).error = Some(error),
        }
    }

    pub(super) fn on_select_terminal(&mut self, id: u64) {
        if self.active_terminal != Some(id) && self.terminal_tabs.iter().any(|tab| tab.id == id) {
            self.active_terminal = Some(id);
            self.terminal_revision = self.terminal_revision.wrapping_add(1);
        }
    }

    pub(super) fn on_close_all_terminals(&mut self) {
        if !self.terminal_tabs.is_empty() {
            self.terminal_tabs.clear();
            self.active_terminal = None;
            self.terminal_revision = self.terminal_revision.wrapping_add(1);
        }
    }

    pub(super) fn on_terminal_started(
        &mut self,
        id: u64,
        result: Result<(TerminalSession, std::sync::mpsc::Receiver<TerminalEvent>), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((session, events)) => {
                self.attach_terminal_session(id, session, events, sender);
            }
            Err(error) => {
                if let Some(tab) = self.terminal_tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.starting = false;
                    tab.exited = true;
                    tab.screen
                        .borrow_mut()
                        .process(format!("\r\nCould not open terminal: {error}\r\n").as_bytes());
                    self.terminal_revision = self.terminal_revision.wrapping_add(1);
                }
            }
        }
    }

    pub(super) fn on_terminal_event(&mut self, id: u64, event: TerminalEvent) {
        match event {
            TerminalEvent::Output(bytes) => {
                if let Some(tab) = self.terminal_tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.screen.borrow_mut().process(&bytes);
                }
            }
            TerminalEvent::Exited => {
                if let Some(tab) = self.terminal_tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.session.take();
                    tab.starting = false;
                    tab.exited = true;
                    tab.screen
                        .borrow_mut()
                        .process(b"\r\n\x1b[90m[terminal exited]\x1b[0m\r\n");
                    self.terminal_revision = self.terminal_revision.wrapping_add(1);
                }
            }
        }
    }

    pub(super) fn on_terminal_input(&mut self, bytes: Vec<u8>) {
        if let Some(active) = self.active_terminal
            && let Some(tab) = self.terminal_tabs.iter().find(|tab| tab.id == active)
        {
            tab.screen
                .borrow_mut()
                .parser
                .screen_mut()
                .set_scrollback(0);
            if let Some(session) = tab.session.as_ref() {
                session.send(&bytes);
            }
        }
    }

    pub(super) fn on_resize_terminal(&mut self, id: u64, rows: u16, cols: u16) {
        if let Some(tab) = self.terminal_tabs.iter().find(|tab| tab.id == id) {
            let resized = tab.screen.borrow_mut().resize(rows, cols);
            if resized && let Some(session) = tab.session.as_ref() {
                session.resize(rows, cols);
            }
        }
    }
}
