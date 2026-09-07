//! Widget-tree updates driven from the component's `update_view` pass.

use super::*;

impl AppWidgets {
    /// Marks whichever sidebar rows point at the directory the active pane is showing.
    ///
    /// This is the sidebar's orientation cue: without it the list cannot say where you are.
    pub(super) fn render_sidebar_active(&mut self, active: &VPath) {
        if self.rendered_active_location.as_ref() == Some(active) {
            return;
        }
        self.rendered_active_location = Some(active.clone());
        // Exactly one row lights up. A location can appear as a place, a favorite and a
        // recent at once; marking all three would read as three separate cues.
        let mut rows = self
            .sidebar_places
            .iter()
            .chain(&self.sidebar_bookmark_rows)
            .chain(&self.sidebar_recent_rows);
        let mut marked = false;
        for (path, button) in rows.by_ref() {
            if !marked && path == active {
                marked = true;
                button.add_css_class("sidebar-row-active");
            } else {
                button.remove_css_class("sidebar-row-active");
            }
        }
    }

    pub(super) fn render_sidebar_remotes(
        &mut self,
        remotes: &[String],
        sender: &ComponentSender<AppModel>,
    ) {
        if self.rendered_remotes == remotes {
            return;
        }
        self.rendered_remotes = remotes.to_vec();
        while let Some(child) = self.sidebar_remotes.first_child() {
            self.sidebar_remotes.remove(&child);
        }
        for (index, uri) in remotes.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            let label = uri
                .split_once("://")
                .map_or(uri.as_str(), |(_, endpoint)| endpoint);
            let button = sidebar_button(label, "commander-server-symbolic");
            button.set_hexpand(true);
            button.set_tooltip_text(Some(uri));
            let input = sender.input_sender().clone();
            let destination = uri.clone();
            button.connect_clicked(move |_| {
                let _ = input.send(AppMsg::ConnectRemote(destination.clone()));
            });
            let remove = icon_button("commander-trash-symbolic", "Forget remote connection");
            remove.add_css_class("flat");
            remove.add_css_class("sidebar-remove");
            let input = sender.input_sender().clone();
            remove.connect_clicked(move |_| {
                let _ = input.send(AppMsg::RemoveRemote(index));
            });
            row.append(&button);
            row.append(&remove);
            self.sidebar_remotes.append(&row);
        }
    }

    pub(super) fn render_sidebar_workspaces(
        &mut self,
        workspaces: &[WorkspaceSession],
        sender: &ComponentSender<AppModel>,
    ) {
        let rendered = workspaces
            .iter()
            .map(|workspace| format!("{workspace:?}"))
            .collect::<Vec<_>>();
        if self.rendered_workspaces == rendered {
            return;
        }
        self.rendered_workspaces = rendered;
        while let Some(child) = self.sidebar_workspaces.first_child() {
            self.sidebar_workspaces.remove(&child);
        }
        if workspaces.is_empty() {
            let empty = gtk::Label::new(Some("No saved workspaces"));
            empty.add_css_class("dim-label");
            empty.add_css_class("sidebar-empty");
            empty.set_xalign(0.0);
            empty.set_wrap(true);
            self.sidebar_workspaces.append(&empty);
            return;
        }
        for (index, workspace) in workspaces.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            row.add_css_class("sidebar-workspace-row");
            let button = sidebar_button(&workspace.name, "commander-panels-top-left-symbolic");
            button.set_hexpand(true);
            let left_tabs = workspace
                .left_pane
                .as_ref()
                .map_or(1, |pane| pane.tabs.len().max(1));
            let right_tabs = workspace
                .right_pane
                .as_ref()
                .map_or(1, |pane| pane.tabs.len().max(1));
            button.set_tooltip_text(Some(&format!(
                "{} · {} left tab{} · {} right tab{}\nLeft: {}\nRight: {}",
                if workspace.dual_pane.unwrap_or(true) {
                    "Two panels"
                } else {
                    "One panel"
                },
                left_tabs,
                plural(left_tabs),
                right_tabs,
                plural(right_tabs),
                workspace.left,
                workspace.right,
            )));
            let input = sender.input_sender().clone();
            button.connect_clicked(move |_| {
                let _ = input.send(AppMsg::OpenWorkspace(index));
            });

            let (menu, actions) = sidebar_context_menu(&row);

            let open =
                context_menu_item_button("Open Workspace", "commander-folder-open-symbolic", None);
            {
                let input = sender.input_sender().clone();
                let menu = menu.clone();
                open.connect_clicked(move |_| {
                    let _ = input.send(AppMsg::OpenWorkspace(index));
                    menu.popdown();
                });
            }
            actions.append(&open);

            let update = context_menu_item_button(
                "Update from Current Setup…",
                "commander-refresh-cw-symbolic",
                None,
            );
            {
                let input = sender.input_sender().clone();
                let menu = menu.clone();
                let name = workspace.name.clone();
                update.connect_clicked(move |_| {
                    menu.popdown();
                    show_update_workspace_dialog(index, &name, input.clone());
                });
            }
            actions.append(&update);

            let rename =
                context_menu_item_button("Rename…", "commander-file-pen-line-symbolic", None);
            {
                let input = sender.input_sender().clone();
                let menu = menu.clone();
                let name = workspace.name.clone();
                rename.connect_clicked(move |_| {
                    menu.popdown();
                    show_rename_workspace_dialog(index, &name, input.clone());
                });
            }
            actions.append(&rename);

            let remove =
                context_menu_item_button("Delete Workspace…", "commander-trash-symbolic", None);
            remove.add_css_class("destructive-action");
            {
                let input = sender.input_sender().clone();
                let menu = menu.clone();
                let name = workspace.name.clone();
                remove.connect_clicked(move |_| {
                    menu.popdown();
                    show_delete_workspace_dialog(index, &name, input.clone());
                });
            }
            actions.append(&remove);

            let click = gtk::GestureClick::new();
            click.set_button(gdk::BUTTON_SECONDARY);
            {
                let menu = menu.clone();
                click.connect_pressed(move |_, _, x, y| {
                    menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                    menu.popup();
                });
            }
            button.add_controller(click);
            row.append(&button);
            self.sidebar_workspaces.append(&row);
        }
    }

    pub(super) fn render_sidebar_recent(
        &mut self,
        recent: &[VPath],
        sender: &ComponentSender<AppModel>,
    ) {
        let rendered = recent.iter().map(ToString::to_string).collect::<Vec<_>>();
        if self.rendered_recent == rendered {
            return;
        }
        self.rendered_recent.clone_from(&rendered);
        self.sidebar_recent_rows.clear();
        self.rendered_active_location = None;
        while let Some(child) = self.sidebar_recent.first_child() {
            self.sidebar_recent.remove(&child);
        }
        if recent.is_empty() {
            let empty = gtk::Label::new(Some("No recent locations"));
            empty.add_css_class("dim-label");
            empty.add_css_class("sidebar-empty");
            empty.set_xalign(0.0);
            self.sidebar_recent.append(&empty);
            return;
        }
        for path in recent {
            let label = path
                .file_name()
                .and_then(OsStr::to_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("/");
            let button = sidebar_button(label, "commander-rotate-ccw-clock-symbolic");
            button.set_tooltip_text(Some(&path.to_string()));
            self.sidebar_recent_rows
                .push((path.clone(), button.clone()));
            let destination = path.clone();
            let drop_destination = path.clone();
            let input = sender.input_sender().clone();
            button.connect_clicked(move |_| {
                let _ = input.send(AppMsg::NavigateActive(destination.clone()));
            });
            install_file_drop_target(&button, sender, Rc::clone(&self.file_drag_ui), move |_| {
                Some(drop_destination.clone())
            });
            self.sidebar_recent.append(&button);
        }
    }

    pub(super) fn render_sidebar_bookmarks(
        &mut self,
        bookmarks: &[VPath],
        groups: &[FavoriteGroupSession],
        sender: &ComponentSender<AppModel>,
    ) {
        let mut rendered = bookmarks
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        rendered.extend(
            groups
                .iter()
                .map(|group| format!("group:{}:{}", group.name, group.paths.join("\u{1f}"))),
        );
        if self.rendered_bookmarks == rendered {
            return;
        }
        self.rendered_bookmarks.clone_from(&rendered);
        self.sidebar_bookmark_rows.clear();
        self.rendered_active_location = None;
        while let Some(child) = self.sidebar_bookmarks.first_child() {
            self.sidebar_bookmarks.remove(&child);
        }
        if bookmarks.is_empty() && groups.is_empty() {
            let empty = gtk::Label::new(Some("No bookmarks yet"));
            empty.add_css_class("dim-label");
            empty.add_css_class("sidebar-empty");
            empty.set_xalign(0.0);
            self.sidebar_bookmarks.append(&empty);
        }
        for (index, path) in bookmarks.iter().enumerate() {
            let label = path
                .file_name()
                .and_then(OsStr::to_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("/");
            let button = sidebar_button(label, "commander-folder-symbolic");
            button.set_halign(gtk::Align::Fill);
            button.set_tooltip_text(Some(&path.to_string()));
            self.sidebar_bookmark_rows
                .push((path.clone(), button.clone()));
            let destination = path.clone();
            let drop_destination = path.clone();
            let input = sender.input_sender().clone();
            button.connect_clicked(move |_| {
                let _ = input.send(AppMsg::NavigateActive(destination.clone()));
            });
            install_file_drop_target(&button, sender, Rc::clone(&self.file_drag_ui), move |_| {
                Some(drop_destination.clone())
            });
            let drag = gtk::DragSource::new();
            drag.set_actions(gdk::DragAction::MOVE);
            drag.connect_prepare(move |_, _, _| {
                let value = format!("favorite-index:{index}");
                Some(gdk::ContentProvider::for_value(&value.to_value()))
            });
            button.add_controller(drag);
            let drop = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
            {
                let input = sender.input_sender().clone();
                drop.connect_drop(move |_, value, _, _| {
                    let Ok(value) = value.get::<String>() else {
                        return false;
                    };
                    let Some(from) = value
                        .strip_prefix("favorite-index:")
                        .and_then(|source| source.parse::<usize>().ok())
                    else {
                        return false;
                    };
                    let _ = input.send(AppMsg::MoveBookmark {
                        from,
                        before: index,
                    });
                    true
                });
            }
            button.add_controller(drop);
            let (menu, actions) = sidebar_context_menu(&button);
            for (label, icon, message) in [
                (
                    "Move Up",
                    "commander-arrow-up-symbolic",
                    (index > 0).then_some(AppMsg::MoveBookmark {
                        from: index,
                        before: index.saturating_sub(1),
                    }),
                ),
                (
                    "Move Down",
                    "commander-chevron-down-symbolic",
                    (index + 1 < bookmarks.len()).then(|| {
                        if index + 2 < bookmarks.len() {
                            AppMsg::MoveBookmark {
                                from: index,
                                before: index + 2,
                            }
                        } else {
                            AppMsg::MoveBookmarkToEnd(index)
                        }
                    }),
                ),
                (
                    "Remove from Favorites",
                    "commander-trash-symbolic",
                    Some(AppMsg::RemoveBookmark(index)),
                ),
            ] {
                let action = context_menu_item_button(label, icon, None);
                action.set_sensitive(message.is_some());
                if let Some(message) = message {
                    let input = sender.input_sender().clone();
                    let menu = menu.clone();
                    let message = Rc::new(RefCell::new(Some(message)));
                    action.connect_clicked(move |_| {
                        if let Some(message) = message.borrow_mut().take() {
                            let _ = input.send(message);
                        }
                        menu.popdown();
                    });
                }
                actions.append(&action);
            }
            let click = gtk::GestureClick::new();
            click.set_button(3);
            {
                let menu = menu.clone();
                click.connect_pressed(move |_, _, x, y| {
                    menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                    menu.popup();
                });
            }
            button.add_controller(click);
            self.sidebar_bookmarks.append(&button);
        }
        for (group_index, group) in groups.iter().enumerate() {
            let heading = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            heading.add_css_class("favorite-group-heading");
            let label = gtk::Label::new(Some(&group.name));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(SIDEBAR_LABEL_WIDTH_CHARS);
            label.set_single_line_mode(true);
            let add = icon_button(
                "commander-plus-symbolic",
                "Add current folder to this group",
            );
            add.add_css_class("flat");
            add.add_css_class("favorite-group-action");
            let input = sender.input_sender().clone();
            add.connect_clicked(move |_| {
                let _ = input.send(AppMsg::AddCurrentToFavoriteGroup(group_index));
            });
            let remove = icon_button("commander-trash-symbolic", "Remove favorite group");
            remove.add_css_class("flat");
            remove.add_css_class("favorite-group-action");
            let input = sender.input_sender().clone();
            remove.connect_clicked(move |_| {
                let _ = input.send(AppMsg::RemoveFavoriteGroup(group_index));
            });
            heading.append(&label);
            heading.append(&add);
            heading.append(&remove);
            self.sidebar_bookmarks.append(&heading);
            if group.paths.is_empty() {
                let empty = gtk::Label::new(Some("Add the active folder with +"));
                empty.add_css_class("dim-label");
                empty.add_css_class("favorite-group-empty");
                empty.set_xalign(0.0);
                self.sidebar_bookmarks.append(&empty);
            }
            for (item_index, path) in group.paths.iter().enumerate() {
                let path = VPath::from(path.as_str());
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
                let label = path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("/");
                let button = sidebar_button(label, "commander-folder-symbolic");
                button.set_hexpand(true);
                button.set_tooltip_text(Some(&path.to_string()));
                let destination = path.clone();
                let drop_destination = path.clone();
                let input = sender.input_sender().clone();
                button.connect_clicked(move |_| {
                    let _ = input.send(AppMsg::NavigateActive(destination.clone()));
                });
                install_file_drop_target(
                    &button,
                    sender,
                    Rc::clone(&self.file_drag_ui),
                    move |_| Some(drop_destination.clone()),
                );
                let remove = icon_button("commander-x-symbolic", "Remove from group");
                remove.add_css_class("flat");
                remove.add_css_class("sidebar-remove");
                let input = sender.input_sender().clone();
                remove.connect_clicked(move |_| {
                    let _ = input.send(AppMsg::RemoveGroupedFavorite {
                        group: group_index,
                        item: item_index,
                    });
                });
                row.append(&button);
                row.append(&remove);
                self.sidebar_bookmarks.append(&row);
            }
        }
    }

    pub(super) fn render_palette(&mut self, model: &AppModel, sender: &ComponentSender<AppModel>) {
        if !model.palette_open {
            if self.palette_pending_open.get() {
                return;
            }
            self.palette_input_active.set(false);
            if self.palette_presented.replace(false) {
                self.palette_dialog.close();
            }
            if !self.palette_input_active.get() && !self.palette_entry.text().is_empty() {
                self.palette_entry.set_text("");
            }
            self.rendered_palette = None;
            return;
        }
        let newly_presented = !self.palette_presented.get();
        self.palette_pending_open.set(false);
        if newly_presented {
            self.palette_input_active.set(true);
            self.palette_dialog.present(Some(&self.palette_parent));
            self.palette_presented.set(true);
        }
        if !self.palette_entry.has_focus() {
            self.palette_entry.grab_focus();
        }
        let rendered = (
            model.palette_query.clone(),
            model.keymap.profile(),
            model.palette_selection,
        );
        if self.rendered_palette.as_ref() == Some(&rendered) {
            return;
        }
        self.rendered_palette = Some(rendered);
        while let Some(child) = self.palette_commands.first_child() {
            self.palette_commands.remove(&child);
        }
        let items = model.palette_items();
        let visible_rows = items.len().min(7) as i32;
        self.palette_dialog
            .set_content_height((112 + visible_rows * 41).max(190));
        if items.is_empty() {
            let empty = gtk::Label::new(Some("No matching commands"));
            empty.add_css_class("command-palette-empty");
            empty.set_vexpand(true);
            self.palette_commands.append(&empty);
            return;
        }
        for (index, item) in items.into_iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let (title, detail) = match item.label.split_once(" · ") {
                Some((title, detail)) => (title, Some(detail)),
                None => (item.label.as_str(), None),
            };
            let label = gtk::Label::new(Some(title));
            label.set_xalign(0.0);
            label.add_css_class("command-row-label");
            row.append(&label);
            let detail_label = gtk::Label::new(detail);
            detail_label.set_xalign(0.0);
            detail_label.set_hexpand(true);
            detail_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            detail_label.add_css_class("command-row-detail");
            row.append(&detail_label);
            if !item.binding.is_empty() {
                row.append(&binding_keycaps(&item.binding));
            }
            let button = gtk::Button::new();
            button.add_css_class("flat");
            button.add_css_class("command-row");
            if index == model.palette_selection {
                button.add_css_class("command-row-selected");
            }
            button.set_child(Some(&row));
            let input = sender.input_sender().clone();
            button.connect_clicked(move |_| {
                let _ = input.send(AppMsg::ActivatePaletteItem(index));
            });
            self.palette_commands.append(&button);
        }
        let adjustment = self.palette_scroll.vadjustment();
        let selected = model.palette_selection;
        glib::idle_add_local_once(move || {
            let row_height = 41.0;
            let row_top = selected as f64 * row_height;
            let row_bottom = row_top + row_height;
            let visible_top = adjustment.value();
            let visible_bottom = visible_top + adjustment.page_size();
            if row_top < visible_top {
                adjustment.set_value(row_top);
            } else if row_bottom > visible_bottom {
                adjustment.set_value((row_bottom - adjustment.page_size()).max(0.0));
            }
        });
    }
}

/// Creates sidebar popovers with the same surface and action insets as file menus.
fn sidebar_context_menu(parent: &impl IsA<gtk::Widget>) -> (gtk::Popover, gtk::Box) {
    let menu = gtk::Popover::new();
    menu.add_css_class("file-context-menu");
    menu.set_autohide(true);
    menu.set_has_arrow(false);
    menu.set_parent(parent);

    let actions = gtk::Box::new(gtk::Orientation::Vertical, 0);
    actions.set_margin_top(6);
    actions.set_margin_bottom(6);
    actions.set_margin_start(6);
    actions.set_margin_end(6);
    menu.set_child(Some(&actions));
    (menu, actions)
}

/// Renders an accelerator label such as `Ctrl+Shift+P / F1` as individual key caps.
pub(super) fn binding_keycaps(binding: &str) -> gtk::Box {
    let keys = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    keys.add_css_class("command-row-keys");
    keys.set_valign(gtk::Align::Center);
    for (alternative_index, alternative) in binding.split(" / ").enumerate() {
        if alternative_index > 0 {
            let separator = gtk::Label::new(Some("or"));
            separator.add_css_class("command-row-key-sep");
            keys.append(&separator);
        }
        let mut parts = alternative
            .split('+')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if alternative.ends_with('+') {
            parts.push("+".to_owned());
        }
        for part in parts {
            let cap = gtk::Label::new(Some(&part));
            cap.add_css_class("command-key");
            keys.append(&cap);
        }
    }
    keys
}
