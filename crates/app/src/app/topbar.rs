//! Navigation and named menus remain reachable at every supported window width.
use super::*;

struct CommandControl {
    widget: gtk::Widget,
    command: CommandId,
    label: &'static str,
    shortcut: Option<gtk::Label>,
}

pub(super) struct TopBarWidgets {
    pub(super) root: gtk::Box,
    primary: gtk::Box,
    filter_row: gtk::Box,
    search_container: adw::Clamp,
    pub(super) search: gtk::SearchEntry,
    filter_pane: Rc<Cell<PaneId>>,
    syncing_filter: Rc<Cell<bool>>,
    rendered_filter: Option<(PaneId, String)>,
    window_controls: gtk::Box,
    pub(super) title: gtk::Label,
    location: gtk::Button,
    #[cfg(test)]
    pub(super) back: gtk::Button,
    #[cfg(test)]
    pub(super) forward: gtk::Button,
    #[cfg(test)]
    pub(super) up: gtk::Button,
    pub(super) new_menu: gtk::MenuButton,
    pub(super) view_menu: gtk::MenuButton,
    pub(super) layout_menu: gtk::MenuButton,
    pub(super) more_menu: gtk::MenuButton,
    menu_labels: [gtk::Label; 3],
    view_icon: gtk::Box,
    rendered_view: Option<PaneViewMode>,
    pub(super) view_buttons: [gtk::ToggleButton; 3],
    hidden: gtk::ToggleButton,
    sidebar: gtk::ToggleButton,
    panels: gtk::ToggleButton,
    terminal: gtk::ToggleButton,
    preview: gtk::ToggleButton,
    vertical: gtk::ToggleButton,
    scope: gtk::Label,
    spinner: gtk::Spinner,
    commands: Vec<CommandControl>,
    #[cfg(test)]
    pub(super) recovery: gtk::Button,
    layout: std::cell::Cell<Option<u8>>,
}

fn menu(
    label: &str,
    icon: &str,
) -> (
    gtk::MenuButton,
    gtk::Label,
    gtk::Box,
    gtk::Popover,
    gtk::Box,
) {
    let button = gtk::MenuButton::new();
    button.add_css_class("topbar-menu");
    button.set_tooltip_text(Some(label));
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let icon_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    icon_slot.append(&gtk::Image::from_icon_name(icon));
    let title = gtk::Label::new(Some(label));
    content.append(&icon_slot);
    content.append(&title);
    button.set_child(Some(&content));
    let popover = gtk::Popover::new();
    popover.add_css_class("topbar-popover");
    let items = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .max_content_height(440)
        .propagate_natural_height(true)
        .child(&items)
        .build();
    popover.set_child(Some(&scroll));
    button.set_popover(Some(&popover));
    (button, title, icon_slot, popover, items)
}

fn section(items: &gtk::Box, label: &str) -> gtk::Label {
    if items.first_child().is_some() {
        items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }
    let title = gtk::Label::new(Some(label));
    title.set_xalign(0.0);
    title.add_css_class("topbar-menu-heading");
    items.append(&title);
    title
}

fn command_row(
    items: &gtk::Box,
    popover: &gtk::Popover,
    controls: &mut Vec<CommandControl>,
    input: &relm4::Sender<AppMsg>,
    label: &'static str,
    icon: &str,
    command: CommandId,
) -> gtk::Button {
    let button = context_menu::context_menu_item_button(label, icon, None);
    let shortcut = gtk::Label::new(None);
    shortcut.add_css_class("context-menu-shortcut");
    button
        .child()
        .and_downcast::<gtk::Box>()
        .unwrap()
        .append(&shortcut);
    let input = input.clone();
    let weak_popover = popover.downgrade();
    button.connect_clicked(move |_| {
        let _ = input.send(AppMsg::ExecuteCommand(command));
        if let Some(popover) = weak_popover.upgrade() {
            popover.popdown();
        }
    });
    items.append(&button);
    controls.push(CommandControl {
        widget: button.clone().upcast(),
        command,
        label,
        shortcut: Some(shortcut),
    });
    button
}

fn toggle_row(button: gtk::ToggleButton, label: &'static str) -> gtk::ToggleButton {
    let icon = button.child();
    button.set_child(gtk::Widget::NONE);
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    if let Some(icon) = icon {
        content.append(&icon);
    }
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    let check = gtk::Image::from_icon_name("commander-check-symbolic");
    button
        .bind_property("active", &check, "visible")
        .sync_create()
        .build();
    content.append(&text);
    content.append(&check);
    button.set_child(Some(&content));
    button.add_css_class("topbar-option");
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

impl TopBarWidgets {
    pub(super) fn new(window: &adw::ApplicationWindow, sender: &ComponentSender<AppModel>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("carelo-toolbar");
        let primary = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        primary.add_css_class("toolbar-primary");
        let leading = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        let trailing = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        trailing.set_hexpand(true);
        trailing.set_halign(gtk::Align::End);
        primary.append(&leading);
        primary.append(&trailing);
        // Custom chrome needs a native handle for dragging and titlebar gestures.
        let window_handle = gtk::WindowHandle::new();
        window_handle.set_child(Some(&primary));
        root.append(&window_handle);
        let window_controls = window_controls(window);
        window_controls.add_css_class("toolbar-window-controls");
        leading.append(&window_controls);
        let navigation = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        navigation.add_css_class("topbar-navigation");
        let mut commands = Vec::new();
        let mut navigation_buttons = Vec::new();
        for (label, icon, command) in [
            ("Back", "commander-chevron-left-symbolic", CommandId::Back),
            (
                "Forward",
                "commander-chevron-right-symbolic",
                CommandId::Forward,
            ),
            (
                "Parent folder",
                "commander-arrow-up-symbolic",
                CommandId::Parent,
            ),
            (
                "Refresh",
                "commander-refresh-cw-symbolic",
                CommandId::Refresh,
            ),
        ] {
            let button = icon_button(icon, label);
            button.add_css_class("flat");
            button.add_css_class("toolbar-icon");
            connect_button(&button, sender, move || AppMsg::ExecuteCommand(command));
            navigation.append(&button);
            commands.push(CommandControl {
                widget: button.clone().upcast(),
                command,
                label,
                shortcut: None,
            });
            navigation_buttons.push(button);
        }
        leading.append(&navigation);
        let location = gtk::Button::new();
        location.add_css_class("flat");
        location.add_css_class("toolbar-location");
        // Leave spare title space draggable without increasing the toolbar minimum.
        location.set_hexpand(true);
        location.set_halign(gtk::Align::Start);
        let title = gtk::Label::new(None);
        title.add_css_class("toolbar-title");
        title.set_xalign(0.0);
        title.set_width_chars(1);
        title.set_max_width_chars(32);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        location.set_child(Some(&title));
        connect_button(&location, sender, || {
            AppMsg::ExecuteCommand(CommandId::FocusLocation)
        });
        leading.append(&location);
        let search = gtk::SearchEntry::new();
        search.add_css_class("toolbar-search");
        search.set_width_chars(12);
        search.set_max_width_chars(24);
        search.set_hexpand(true);
        let search_container = adw::Clamp::new();
        search_container.set_child(Some(&search));
        search_container.set_maximum_size(560);
        search_container.set_tightening_threshold(360);
        search_container.set_hexpand(true);
        let filter_pane = Rc::new(Cell::new(PaneId::Left));
        let syncing_filter = Rc::new(Cell::new(false));
        let input = sender.input_sender().clone();
        let pane = Rc::clone(&filter_pane);
        let syncing = Rc::clone(&syncing_filter);
        // SearchEntry's delayed search-changed lets unrelated model updates erase
        // pending input. Capture edits immediately, including their original pane.
        search.connect_changed(move |entry| {
            if !syncing.get() {
                let _ = input.send(AppMsg::SetPaneFilter(pane.get(), entry.text().to_string()));
            }
        });
        let input = sender.input_sender().clone();
        let pane = Rc::clone(&filter_pane);
        search.connect_stop_search(move |_| {
            let _ = input.send(AppMsg::SetPaneFilter(pane.get(), String::new()));
            let _ = input.send(AppMsg::ExecuteCommand(CommandId::FocusFiles));
        });
        let filter_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        filter_row.add_css_class("toolbar-filter-row");
        filter_row.append(&search_container);
        root.append(&filter_row);

        let (new_menu, new_label, _, new_popover, new_items) =
            menu("New", "commander-plus-symbolic");
        section(&new_items, "Create in the active folder");
        for (label, icon, command) in [
            (
                "New folder",
                "commander-folder-plus-symbolic",
                CommandId::NewDirectory,
            ),
            (
                "New file",
                "commander-file-plus-symbolic",
                CommandId::NewFile,
            ),
            (
                "Archive selected items…",
                "commander-archive-symbolic",
                CommandId::CreateArchive,
            ),
            (
                "New tab",
                "commander-square-plus-symbolic",
                CommandId::NewTab,
            ),
        ] {
            command_row(
                &new_items,
                &new_popover,
                &mut commands,
                sender.input_sender(),
                label,
                icon,
                command,
            );
        }
        trailing.append(&new_menu);

        let (view_menu, view_label, view_icon, view_popover, view_items) =
            menu("View", "commander-list-symbolic");
        let scope = section(&view_items, "View for the active pane");
        let list = toggle_row(
            view_toggle_button("commander-list-symbolic", "List"),
            "List",
        );
        let grid = toggle_row(
            view_toggle_button("commander-layout-grid-symbolic", "Grid"),
            "Grid",
        );
        let columns = toggle_row(column_view_toggle_button("Columns"), "Columns");
        grid.set_group(Some(&list));
        columns.set_group(Some(&list));
        for (button, mode) in [
            (&list, PaneViewMode::List),
            (&grid, PaneViewMode::Grid),
            (&columns, PaneViewMode::Columns),
        ] {
            let input = sender.input_sender().clone();
            let popover = view_popover.downgrade();
            button.connect_clicked(move |_| {
                let _ = input.send(AppMsg::SetViewMode(mode));
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
            });
            view_items.append(button);
        }
        section(&view_items, "Visibility");
        let hidden = toggle_row(
            view_toggle_button("commander-eye-symbolic", "Show hidden files"),
            "Show hidden files",
        );
        connect_button(hidden.upcast_ref(), sender, || {
            AppMsg::ExecuteCommand(CommandId::ToggleHidden)
        });
        commands.push(CommandControl {
            widget: hidden.clone().upcast(),
            command: CommandId::ToggleHidden,
            label: "Show hidden files",
            shortcut: None,
        });
        view_items.append(&hidden);
        trailing.append(&view_menu);

        let (layout_menu, layout_label, _, _, layout_items) =
            menu("Layout", "commander-columns-2-symbolic");
        section(&layout_items, "Panels and tools");
        let sidebar = toggle_row(
            view_toggle_button("commander-panel-left-symbolic", "Locations sidebar"),
            "Locations sidebar",
        );
        let panels = toggle_row(
            pane_count_toggle_button("Two file panels"),
            "Two file panels",
        );
        let preview = toggle_row(
            view_toggle_button("commander-panel-right-symbolic", "Inspector"),
            "Inspector",
        );
        let terminal = toggle_row(
            view_toggle_button("commander-terminal-symbolic", "Terminal"),
            "Terminal",
        );
        let vertical = toggle_row(
            view_toggle_button("commander-rotate-cw-symbolic", "Stack panels vertically"),
            "Stack panels vertically",
        );
        for (button, label, command) in [
            (&sidebar, "Locations sidebar", CommandId::ToggleSidebar),
            (&panels, "Two file panels", CommandId::ToggleDualPane),
            (&preview, "Inspector", CommandId::TogglePreview),
            (&terminal, "Terminal", CommandId::ToggleTerminal),
            (
                &vertical,
                "Stack panels vertically",
                CommandId::ToggleOrientation,
            ),
        ] {
            connect_button(button.upcast_ref(), sender, move || {
                AppMsg::ExecuteCommand(command)
            });
            commands.push(CommandControl {
                widget: button.clone().upcast(),
                command,
                label,
                shortcut: None,
            });
            layout_items.append(button);
        }
        trailing.append(&layout_menu);

        let (more_menu, more_label, _, more_popover, more_items) =
            menu("More actions", "commander-ellipsis-symbolic");
        more_label.set_visible(false);
        section(&more_items, "Find and navigate");
        for (label, icon, command) in [
            (
                "Search files…",
                "commander-search-symbolic",
                CommandId::UnifiedSearch,
            ),
            (
                "Command palette…",
                "commander-menu-symbolic",
                CommandId::CommandPalette,
            ),
        ] {
            command_row(
                &more_items,
                &more_popover,
                &mut commands,
                sender.input_sender(),
                label,
                icon,
                command,
            );
        }
        section(&more_items, "File operations");
        for (label, icon, command) in [
            (
                "Copy to other pane",
                "commander-copy-symbolic",
                CommandId::Copy,
            ),
            (
                "Move to other pane",
                "commander-corner-up-right-symbolic",
                CommandId::Move,
            ),
            (
                "Move to Trash",
                "commander-trash-symbolic",
                CommandId::Trash,
            ),
            ("Undo", "commander-undo-2-symbolic", CommandId::Undo),
            ("Redo", "commander-redo-2-symbolic", CommandId::Redo),
        ] {
            command_row(
                &more_items,
                &more_popover,
                &mut commands,
                sender.input_sender(),
                label,
                icon,
                command,
            );
        }
        let recovery = context_menu::context_menu_item_button(
            "Recovery and saved operations…",
            "commander-rotate-ccw-clock-symbolic",
            None,
        );
        let input = sender.input_sender().clone();
        let popover = more_popover.downgrade();
        recovery.connect_clicked(move |_| {
            let _ = input.send(AppMsg::ShowRecovery);
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
        });
        more_items.append(&recovery);
        section(&more_items, "File tools");
        for (label, icon, command) in [
            (
                "Batch rename…",
                "commander-file-pen-line-symbolic",
                CommandId::BatchRename,
            ),
            (
                "Find duplicate files…",
                "commander-search-symbolic",
                CommandId::FindDuplicates,
            ),
            (
                "Compare files…",
                "commander-file-symbolic",
                CommandId::CompareFiles,
            ),
            (
                "Compare and sync folders…",
                "commander-folder-symbolic",
                CommandId::CompareDirectories,
            ),
        ] {
            command_row(
                &more_items,
                &more_popover,
                &mut commands,
                sender.input_sender(),
                label,
                icon,
                command,
            );
        }
        section(&more_items, "Application");
        command_row(
            &more_items,
            &more_popover,
            &mut commands,
            sender.input_sender(),
            "Settings…",
            "commander-settings-2-symbolic",
            CommandId::Settings,
        );
        trailing.append(&more_menu);
        let spinner = gtk::Spinner::new();
        spinner.set_tooltip_text(Some("Loading folder contents"));
        spinner.set_visible(false);
        trailing.append(&spinner);
        let result = Self {
            root,
            primary,
            filter_row,
            search_container,
            search,
            filter_pane,
            syncing_filter,
            rendered_filter: None,
            window_controls,
            title,
            location,
            #[cfg(test)]
            back: navigation_buttons[0].clone(),
            #[cfg(test)]
            forward: navigation_buttons[1].clone(),
            #[cfg(test)]
            up: navigation_buttons[2].clone(),
            new_menu,
            view_menu,
            layout_menu,
            more_menu,
            menu_labels: [new_label, view_label, layout_label],
            view_icon,
            rendered_view: None,
            view_buttons: [list, grid, columns],
            hidden,
            sidebar,
            panels,
            terminal,
            preview,
            vertical,
            scope,
            spinner,
            commands,
            #[cfg(test)]
            recovery,
            layout: std::cell::Cell::new(None),
        };
        result.apply_layout(0);
        result
    }

    pub(super) fn apply_layout(&self, width: i32) {
        let layout = if width >= 1_000 {
            0
        } else if width >= 600 {
            1
        } else {
            2
        };
        if self.layout.get() == Some(layout) {
            return;
        }
        self.layout.set(Some(layout));
        if layout == 2 {
            self.root.add_css_class("compact-topbar");
        } else {
            self.root.remove_css_class("compact-topbar");
        }
        for label in &self.menu_labels {
            label.set_visible(layout < 2);
        }
        let inline = layout == 0;
        // Three equal regions guarantee centering and prevent either side from
        // taking the filter's space. The title ellipsizes within its region.
        self.primary.set_homogeneous(inline);
        let currently_inline =
            self.search_container.parent().as_ref() == Some(self.primary.upcast_ref());
        if currently_inline != inline {
            let focus = self
                .root
                .root()
                .and_downcast::<gtk::Window>()
                .and_then(|window| gtk::prelude::GtkWindowExt::focus(&window));
            let focused = focus.is_some_and(|focus| focus.is_ancestor(&self.search));
            let selection = self.search.selection_bounds();
            let cursor = self.search.position();
            if inline {
                self.filter_row.remove(&self.search_container);
                self.primary.insert_child_after(
                    &self.search_container,
                    self.primary.first_child().as_ref(),
                );
            } else {
                self.primary.remove(&self.search_container);
                self.filter_row.set_visible(true);
                self.filter_row.append(&self.search_container);
            }
            if focused {
                self.search.grab_focus();
                if let Some((start, end)) = selection {
                    self.search.select_region(start, end);
                } else {
                    self.search.set_position(cursor);
                }
            }
        }
        self.filter_row.set_visible(!inline);
    }

    pub(super) fn render(&mut self, model: &AppModel) {
        // Use one layout policy, based on the space actually available to the bar.
        // Window allocation reflects tiling; saved geometry may still be wider.
        // Subtract the requested sidebar state so toggling it updates immediately.
        let available = self
            .root
            .root()
            .map_or(0, |window| window.width())
            .saturating_sub(i32::from(model.sidebar_visible) * SIDEBAR_WIDTH);
        self.window_controls.set_visible(!model.sidebar_visible);
        self.apply_layout(available);
        let state = model.pane(model.active_pane);
        let display_path = model.archive_mounts.display(state.current_directory());
        let path = &display_path;
        self.filter_pane.set(model.active_pane);
        let filter = (model.active_pane, state.filter_query.clone());
        if self.rendered_filter.as_ref() != Some(&filter) {
            self.rendered_filter = Some(filter);
            self.syncing_filter.set(true);
            if self.search.text().as_str() != state.filter_query {
                self.search.set_text(&state.filter_query);
            }
            self.syncing_filter.set(false);
        }
        self.new_menu
            .set_tooltip_text(Some(&format!("Create in {path}")));
        self.layout_menu.set_tooltip_text(Some(if model.dual_pane {
            "Layout options · two file panels"
        } else {
            "Layout options · one file panel"
        }));
        self.more_menu.set_tooltip_text(Some(&format!(
            "More actions · command palette {}",
            model.keymap.binding_label(CommandId::CommandPalette)
        )));
        self.title.set_label(
            &path
                .file_name()
                .map_or_else(|| "File System".to_owned(), display_name),
        );
        let location_tip = format!(
            "{}\nEdit location · {}",
            path,
            model.keymap.binding_label(CommandId::FocusLocation)
        );
        self.location.set_tooltip_text(Some(&location_tip));
        self.location
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Edit location: {path}"
            ))]);
        for (button, mode) in self.view_buttons.iter().zip([
            PaneViewMode::List,
            PaneViewMode::Grid,
            PaneViewMode::Columns,
        ]) {
            button.set_active(state.view_mode == mode);
        }
        if self.rendered_view != Some(state.view_mode) {
            self.rendered_view = Some(state.view_mode);
            let label = match state.view_mode {
                PaneViewMode::List => "List",
                PaneViewMode::Grid => "Grid",
                PaneViewMode::Columns => "Columns",
            };
            self.menu_labels[1].set_label(label);
            while let Some(child) = self.view_icon.first_child() {
                self.view_icon.remove(&child);
            }
            if state.view_mode == PaneViewMode::Columns {
                let button = column_view_toggle_button("Columns");
                let icon = button.child().unwrap();
                button.set_child(gtk::Widget::NONE);
                self.view_icon.append(&icon);
            } else {
                self.view_icon.append(&gtk::Image::from_icon_name(
                    if state.view_mode == PaneViewMode::List {
                        "commander-list-symbolic"
                    } else {
                        "commander-layout-grid-symbolic"
                    },
                ));
            }
            self.view_menu
                .set_tooltip_text(Some(&format!("View options · {label}")));
        }
        self.hidden.set_active(state.show_hidden);
        self.sidebar.set_active(model.sidebar_visible);
        self.panels.set_active(model.dual_pane);
        self.terminal.set_active(model.terminal_visible);
        self.preview.set_active(model.preview_visible);
        self.vertical.set_active(model.vertical_split);
        self.vertical.set_sensitive(model.dual_pane);
        let pane = if model.dual_pane {
            format!("{} pane", model.active_pane.label())
        } else {
            "current folder".to_owned()
        };
        self.scope.set_label(&format!("View for {pane}"));
        self.search
            .set_placeholder_text(Some(&format!("Filter {pane}…")));
        self.search.set_tooltip_text(Some(&format!(
            "Filter names in {path} · {}\nEscape clears the filter",
            model.keymap.binding_label(CommandId::FocusFilter)
        )));
        self.search
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Filter files in {pane}"
            ))]);
        if state.filter_query.is_empty() {
            self.search.remove_css_class("has-filter");
        } else {
            self.search.add_css_class("has-filter");
        }
        let context = model.action_context(model.active_pane);
        for control in &self.commands {
            let binding = model.keymap.binding_label(control.command);
            let decision = context.action(control.command);
            let label = decision.label(control.label);
            if control.command == CommandId::Trash {
                // This row uses the shared context-menu icon, title, shortcut layout.
                if let Some(title) = control
                    .widget
                    .first_child()
                    .and_then(|content| content.first_child())
                    .and_then(|icon| icon.next_sibling())
                    .and_downcast::<gtk::Label>()
                {
                    title.set_label(label);
                }
                control
                    .widget
                    .update_property(&[gtk::accessible::Property::Label(label)]);
            }
            let tip = if let Some(reason) = decision.reason {
                format!("{label}: {reason}")
            } else if binding.is_empty() {
                label.to_owned()
            } else {
                format!("{label} · {binding}")
            };
            if control.widget.tooltip_text().as_deref() != Some(&tip) {
                control.widget.set_tooltip_text(Some(&tip));
            }
            if let Some(shortcut) = &control.shortcut {
                shortcut.set_label(&binding);
            }
            control.widget.set_sensitive(decision.enabled());
        }
        self.spinner.set_visible(state.loading || state.filtering);
        self.spinner.set_spinning(state.loading || state.filtering);
    }
}
