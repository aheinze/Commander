use super::*;

#[derive(Clone, Debug)]
pub(super) struct Action {
    pub(super) label: &'static str,
    icon: &'static str,
    pub(super) command: CommandId,
}

fn action(label: &'static str, icon: &'static str, command: CommandId) -> Action {
    Action {
        label,
        icon,
        command,
    }
}

pub(super) fn sections(
    target: Option<&(VPath, EntryKind)>,
    selected: usize,
) -> Vec<(bool, Vec<Action>)> {
    let mut groups = Vec::new();
    let Some((path, kind)) = target else {
        return vec![
            (
                false,
                vec![
                    action(
                        "New folder…",
                        "commander-folder-plus-symbolic",
                        CommandId::NewDirectory,
                    ),
                    action(
                        "New file…",
                        "commander-file-plus-symbolic",
                        CommandId::NewFile,
                    ),
                ],
            ),
            (
                false,
                vec![action(
                    "Paste",
                    "commander-clipboard-symbolic",
                    CommandId::Paste,
                )],
            ),
            (
                false,
                vec![
                    action(
                        "Open in new tab",
                        "commander-square-plus-symbolic",
                        CommandId::NewTab,
                    ),
                    action(
                        "Copy folder path",
                        "commander-copy-symbolic",
                        CommandId::CopyDirectoryPath,
                    ),
                    action(
                        "Refresh folder",
                        "commander-refresh-cw-symbolic",
                        CommandId::Refresh,
                    ),
                ],
            ),
        ];
    };
    if selected == 1 {
        let archive = !kind.is_directory() && is_archive_path(path);
        let mut open = vec![action(
            if archive { "Browse archive" } else { "Open" },
            if archive {
                "commander-archive-symbolic"
            } else if kind.is_directory() {
                "commander-folder-symbolic"
            } else {
                "commander-file-symbolic"
            },
            if archive {
                CommandId::BrowseArchive
            } else {
                CommandId::Open
            },
        )];
        if kind.is_directory() || archive {
            open.push(action(
                "Open in new tab",
                "commander-square-plus-symbolic",
                CommandId::OpenInNewTab,
            ));
        }
        if !kind.is_directory() {
            open.push(action(
                "Quick Look",
                "commander-eye-symbolic",
                CommandId::QuickLook,
            ));
            open.push(action(
                "Open with…",
                "commander-app-window-symbolic",
                CommandId::OpenWith,
            ));
        }
        groups.push((false, open));
    }
    let mut edit = vec![
        action("Cut", "commander-scissors-symbolic", CommandId::Cut),
        action("Copy", "commander-copy-symbolic", CommandId::CopyClipboard),
    ];
    if selected == 1 && kind.is_directory() {
        edit.push(action(
            "Paste into folder",
            "commander-clipboard-symbolic",
            CommandId::Paste,
        ));
    }
    edit.push(if selected > 1 {
        action(
            "Rename selected items…",
            "commander-file-pen-line-symbolic",
            CommandId::BatchRename,
        )
    } else {
        action(
            "Rename…",
            "commander-file-pen-line-symbolic",
            CommandId::Rename,
        )
    });
    groups.push((false, edit));
    groups.push((
        false,
        vec![
            action(
                "Copy to other pane",
                "commander-copy-symbolic",
                CommandId::Copy,
            ),
            action(
                "Move to other pane",
                "commander-corner-up-right-symbolic",
                CommandId::Move,
            ),
        ],
    ));
    groups.push((
        false,
        vec![action(
            "Move to Trash",
            "commander-trash-symbolic",
            CommandId::Trash,
        )],
    ));

    let mut location = vec![action(
        if selected > 1 {
            "Copy paths"
        } else {
            "Copy path"
        },
        "commander-copy-symbolic",
        CommandId::CopyPath,
    )];
    if selected == 1 {
        location.push(action(
            "Open in other pane",
            "commander-panel-right-symbolic",
            CommandId::OpenOtherPane,
        ));
        location.push(action(
            "Reveal in file manager",
            "commander-folder-symbolic",
            CommandId::Reveal,
        ));
        if kind.is_directory() {
            location.push(action(
                "Quick Look",
                "commander-eye-symbolic",
                CommandId::QuickLook,
            ));
        }
    }
    groups.push((true, location));
    let mut tools = Vec::new();
    if selected == 1 {
        if *kind == EntryKind::File {
            tools.push(action(
                "Edit file",
                "commander-file-pen-line-symbolic",
                CommandId::EditFile,
            ));
            tools.push(action(
                "Verify checksum…",
                "commander-file-key-symbolic",
                CommandId::Checksum,
            ));
            if path
                .as_path()
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            {
                tools.push(action(
                    "PDF tools…",
                    "commander-file-text-symbolic",
                    CommandId::PdfTools,
                ));
            }
            if supports_image_conversion(path, *kind) {
                tools.push(action(
                    "Convert image…",
                    "commander-file-image-symbolic",
                    CommandId::ConvertImage,
                ));
            }
        }
        tools.push(action(
            "Permissions…",
            "commander-key-round-symbolic",
            CommandId::Permissions,
        ));
        tools.push(action(
            "Batch rename…",
            "commander-file-pen-line-symbolic",
            CommandId::BatchRename,
        ));
    }
    tools.push(action(
        "Find duplicate files…",
        "commander-search-symbolic",
        CommandId::FindDuplicates,
    ));
    if *kind == EntryKind::File {
        tools.push(action(
            "Compare files…",
            "commander-file-symbolic",
            CommandId::CompareFiles,
        ));
    }
    groups.push((true, tools));
    let mut archives = vec![action(
        "Create archive…",
        "commander-archive-symbolic",
        CommandId::CreateArchive,
    )];
    if selected == 1 && *kind == EntryKind::File && is_archive_path(path) {
        archives.push(action(
            "Edit archive contents…",
            "commander-archive-symbolic",
            CommandId::EditArchive,
        ));
        archives.push(action(
            "Extract archive",
            "commander-archive-symbolic",
            CommandId::ExtractArchive,
        ));
    }
    groups.push((true, archives));
    let mut destructive = vec![action(
        "Delete permanently…",
        "commander-trash-symbolic",
        CommandId::DeletePermanent,
    )];
    if *kind == EntryKind::File && path.as_path().is_absolute() {
        destructive.push(action(
            "Secure delete…",
            "commander-trash-symbolic",
            CommandId::SecureDelete,
        ));
    }
    groups.push((true, destructive));
    groups
}

fn tool_applies(tool: &CustomToolSession, path: &VPath, kind: EntryKind) -> bool {
    tool.enabled
        && !(tool.applies_to == "files" && kind != EntryKind::File)
        && !(tool.applies_to == "folders" && kind != EntryKind::Directory)
        && (tool.extensions.trim().is_empty() || kind != EntryKind::File || {
            let extension = path
                .as_path()
                .extension()
                .and_then(OsStr::to_str)
                .unwrap_or_default();
            tool.extensions
                .split([',', ' ', ';'])
                .filter(|value| !value.is_empty())
                .any(|value| {
                    value
                        .trim_start_matches('.')
                        .eq_ignore_ascii_case(extension)
                })
        })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_menu(
    parent: &gtk::Widget,
    pane: PaneId,
    target: Option<&(VPath, EntryKind)>,
    selected: usize,
    keymap: &Keymap,
    tools: &[CustomToolSession],
    input: &relm4::Sender<AppMsg>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("file-context-menu");
    popover.set_autohide(true);
    popover.set_has_arrow(false);
    popover.set_parent(parent);
    let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    title.add_css_class("context-menu-title");
    let text = if selected > 1 {
        format!("{selected} items selected")
    } else {
        target.and_then(|(path, _)| path.file_name()).map_or_else(
            || "Current folder".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    let title_label = gtk::Label::new(Some(&text));
    title_label.set_xalign(0.0);
    title_label.set_hexpand(true);
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    title_label.set_max_width_chars(28);
    title_label.set_tooltip_text(Some(&text));
    title.append(&title_label);
    let kind = gtk::Label::new(Some(if selected > 1 {
        "Selection"
    } else if target.is_none_or(|(_, kind)| kind.is_directory()) {
        "Folder"
    } else {
        "File"
    }));
    kind.add_css_class("dim-label");
    title.append(&kind);
    shell.append(&title);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search all actions…"));
    search.update_property(&[gtk::accessible::Property::Label(
        "Search all context menu actions",
    )]);
    search.add_css_class("context-menu-search");
    search.set_width_chars(16);
    search.set_max_width_chars(28);
    shell.append(&search);

    let back = context_menu_item_button("Back", "commander-chevron-left-symbolic", None);
    back.add_css_class("context-menu-back");
    shell.append(&back);
    let actions = gtk::Box::new(gtk::Orientation::Vertical, 0);
    actions.set_margin_start(5);
    actions.set_margin_end(5);
    actions.set_margin_bottom(5);
    let mut groups = Vec::new();
    let mut secondary = Vec::new();
    let clipboard = parent.clipboard();
    let mut paste_buttons = Vec::new();
    for (more, section) in sections(target, selected) {
        if section.is_empty() {
            continue;
        }
        let rows = section
            .into_iter()
            .map(|spec| {
                let bindings = keymap.binding_label(spec.command);
                let shortcut = bindings.split(" / ").next().unwrap_or_default();
                let row = if spec.command == CommandId::Paste
                    && let Some((destination, _)) = target
                {
                    let destination = destination.clone();
                    let button = context_menu_item_button(
                        spec.label,
                        spec.icon,
                        (!shortcut.is_empty()).then_some(shortcut),
                    );
                    let input = input.clone();
                    let weak = popover.downgrade();
                    button.connect_clicked(move |_| {
                        let _ = input.send(AppMsg::PasteInto(pane, destination.clone()));
                        if let Some(popover) = weak.upgrade() {
                            popover.popdown();
                        }
                    });
                    (button, format!("{} paste", spec.label).to_lowercase())
                } else {
                    context_menu_command_row(
                        spec.label,
                        spec.icon,
                        shortcut,
                        spec.command,
                        matches!(
                            spec.command,
                            CommandId::DeletePermanent | CommandId::SecureDelete
                        ),
                        input,
                        &popover,
                    )
                };
                if spec.command == CommandId::Paste {
                    paste_buttons.push(row.0.clone());
                }
                if !bindings.is_empty() {
                    row.0
                        .set_tooltip_text(Some(&format!("{} · {bindings}", spec.label)));
                }
                row
            })
            .collect();
        append_context_menu_group(&actions, &mut groups, rows);
        secondary.push(more);
    }
    if let Some((path, kind)) = target {
        let rows: Vec<_> = tools
            .iter()
            .enumerate()
            .filter(|(_, tool)| tool_applies(tool, path, *kind))
            .map(|(index, tool)| context_menu_tool_row(&tool.name, index, input, &popover))
            .collect();
        if !rows.is_empty() {
            append_context_menu_group(&actions, &mut groups, rows);
            secondary.push(true);
        }
    }
    let tags = target.map(|_| context_menu_tag_row(input, &popover));
    if let Some(tags) = &tags {
        actions.append(tags);
    }
    let empty = gtk::Label::new(Some("No actions found. Try a different word."));
    empty.add_css_class("context-menu-empty");
    empty.set_wrap(true);
    empty.set_max_width_chars(30);
    actions.append(&empty);
    let more_button = context_menu_item_button("More actions", "commander-ellipsis-symbolic", None);
    more_button.add_css_class("context-menu-more");
    if let Some(content) = more_button.child().and_downcast::<gtk::Box>() {
        content.append(&gtk::Image::from_icon_name(
            "commander-chevron-right-symbolic",
        ));
    }
    actions.append(&more_button);
    let height = parent.root().map_or(720, |root| root.height());
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .overlay_scrolling(false)
        .propagate_natural_height(true)
        .max_content_height((height - 150).clamp(140, 460))
        .child(&actions)
        .build();
    shell.append(&scroll);
    popover.set_child(Some(&shell));
    let content = Rc::new(MenuContent {
        popover: popover.downgrade(),
        search,
        back,
        groups,
        secondary,
        more: Cell::new(false),
        more_button,
        empty,
        tags,
        scroll,
    });
    let weak = Rc::downgrade(&content);
    content.search.connect_changed(move |_| {
        if let Some(content) = weak.upgrade() {
            content.filter();
        }
    });
    let weak = Rc::downgrade(&content);
    content.more_button.connect_clicked(move |_| {
        if let Some(content) = weak.upgrade() {
            content.change_page(true);
        }
    });
    let weak = Rc::downgrade(&content);
    content.back.connect_clicked(move |_| {
        if let Some(content) = weak.upgrade() {
            content.change_page(false);
        }
    });
    let weak = Rc::downgrade(&content);
    popover.connect_map(move |_| {
        let weak = weak.clone();
        glib::idle_add_local_once(move || {
            if let Some(content) = weak.upgrade()
                && content
                    .popover
                    .upgrade()
                    .is_some_and(|menu| menu.is_mapped())
            {
                content.focus_first();
            }
        });
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let keyboard = content.clone();
    keys.connect_key_pressed(move |_, key, _, modifiers| keyboard.key(key, modifiers));
    let weak_keys = keys.downgrade();
    popover.add_controller(keys);
    sync_paste(&clipboard, &paste_buttons);
    let buttons: Vec<_> = paste_buttons.iter().map(gtk::Button::downgrade).collect();
    let handler = clipboard.connect_changed(move |clipboard| {
        sync_paste(
            clipboard,
            &buttons
                .iter()
                .filter_map(glib::WeakRef::upgrade)
                .collect::<Vec<_>>(),
        );
    });
    let handler = Cell::new(Some(handler));
    popover.connect_closed(move |popover| {
        if let Some(handler) = handler.take() {
            clipboard.disconnect(handler);
        }
        if let Some(keys) = weak_keys.upgrade() {
            popover.remove_controller(&keys);
        }
        if let Some(parent) = popover.parent()
            && let Some(window) = popover.root().and_downcast::<gtk::Window>()
            && gtk::prelude::GtkWindowExt::focus(&window)
                .is_some_and(|focus| focus.is_ancestor(popover))
        {
            parent.grab_focus();
        }
        popover.set_child(gtk::Widget::NONE);
        popover.unparent();
    });
    content.filter();
    popover
}

fn sync_paste(clipboard: &gdk::Clipboard, buttons: &[gtk::Button]) {
    let formats = clipboard.formats();
    let available = formats.contains_type(gdk::FileList::static_type())
        || formats.contain_mime_type("text/uri-list")
        || formats.contain_mime_type("x-special/gnome-copied-files");
    for button in buttons {
        button.set_sensitive(available);
        if !available {
            button.set_tooltip_text(Some("Copy or cut files to enable Paste"));
        } else {
            button.set_tooltip_text(Some("Paste files from the clipboard"));
        }
    }
}

struct MenuContent {
    popover: glib::WeakRef<gtk::Popover>,
    search: gtk::SearchEntry,
    back: gtk::Button,
    groups: Vec<ContextMenuFilterGroup>,
    secondary: Vec<bool>,
    more: Cell<bool>,
    more_button: gtk::Button,
    empty: gtk::Label,
    tags: Option<gtk::Box>,
    scroll: gtk::ScrolledWindow,
}

impl MenuContent {
    fn filter(&self) {
        let query = self.search.text();
        let searching = !query.trim().is_empty();
        filter_context_menu(&query, &self.groups);
        let mut found = false;
        for (group, secondary) in self.groups.iter().zip(&self.secondary) {
            let visible =
                group.container.get_visible() && (searching || *secondary == self.more.get());
            group.container.set_visible(visible);
            group.separator.set_visible(visible && found);
            found |= visible;
        }
        if let Some(tags) = &self.tags {
            let mut visible = false;
            let mut child = tags.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                if let Some(button) = widget.downcast_ref::<gtk::Button>() {
                    let matches = if searching {
                        palette_query_matches(&query, &["tags labels colors"])
                            || palette_query_matches(
                                &query,
                                &[button.tooltip_text().as_deref().unwrap_or_default()],
                            )
                    } else {
                        self.more.get()
                    };
                    button.set_visible(matches);
                    visible |= matches;
                }
            }
            tags.set_visible(visible);
            found |= visible;
        }
        self.back.set_visible(self.more.get());
        self.more_button
            .set_visible(!self.more.get() && !searching && self.secondary.contains(&true));
        self.empty.set_visible(!found);
        self.scroll.vadjustment().set_value(0.0);
        if let Some(popover) = self.popover.upgrade()
            && popover.is_mapped()
        {
            popover.present();
        }
    }

    fn change_page(&self, more: bool) {
        self.more.set(more);
        self.search.set_text("");
        self.filter();
        self.focus_first();
    }

    fn buttons(&self) -> Vec<gtk::Button> {
        let mut buttons = Vec::new();
        if self.back.is_visible() {
            buttons.push(self.back.clone());
        }
        for group in &self.groups {
            if group.container.is_visible() {
                buttons.extend(
                    group
                        .rows
                        .iter()
                        .filter(|(button, _)| button.is_visible() && button.is_sensitive())
                        .map(|(button, _)| button.clone()),
                );
            }
        }
        if let Some(tags) = &self.tags
            && tags.is_visible()
        {
            let mut child = tags.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                if let Ok(button) = widget.downcast::<gtk::Button>()
                    && button.is_visible()
                {
                    buttons.push(button);
                }
            }
        }
        if self.more_button.is_visible() {
            buttons.push(self.more_button.clone());
        }
        buttons
    }

    fn focus_button(&self, button: &gtk::Button) {
        button.grab_focus();
        if let Some(actions) = self.scroll.child()
            && let Some(bounds) = button.compute_bounds(&actions)
        {
            let adjustment = self.scroll.vadjustment();
            let top = f64::from(bounds.y());
            let bottom = top + f64::from(bounds.height());
            if top < adjustment.value() {
                adjustment.set_value(top.max(0.0));
            } else if bottom > adjustment.value() + adjustment.page_size() {
                adjustment.set_value(bottom - adjustment.page_size());
            }
        }
    }

    fn focus_first(&self) {
        if let Some(button) = self
            .buttons()
            .into_iter()
            .find(|button| *button != self.back)
        {
            self.focus_button(&button);
        } else {
            self.search.grab_focus();
        }
    }

    fn key(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let Some(popover) = self.popover.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let focus = popover
            .root()
            .and_downcast::<gtk::Window>()
            .and_then(|window| gtk::prelude::GtkWindowExt::focus(&window));
        let editing = focus
            .as_ref()
            .is_some_and(|focus| focus.is_ancestor(&self.search));
        let buttons = self.buttons();
        let current = focus.as_ref().and_then(|focus| {
            buttons.iter().position(|button| {
                focus == button.upcast_ref::<gtk::Widget>() || focus.is_ancestor(button)
            })
        });
        match key {
            gdk::Key::Escape => popover.popdown(),
            gdk::Key::Down | gdk::Key::KP_Down | gdk::Key::Up | gdk::Key::KP_Up => {
                if !buttons.is_empty() {
                    let down = matches!(key, gdk::Key::Down | gdk::Key::KP_Down);
                    let next = current.map_or(if down { 0 } else { buttons.len() - 1 }, |index| {
                        if down {
                            (index + 1) % buttons.len()
                        } else {
                            (index + buttons.len() - 1) % buttons.len()
                        }
                    });
                    self.focus_button(&buttons[next]);
                }
            }
            gdk::Key::Home | gdk::Key::End if !editing => {
                if let Some(button) = if key == gdk::Key::Home {
                    buttons.first()
                } else {
                    buttons.last()
                } {
                    self.focus_button(button);
                }
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                if let Some(index) = current {
                    buttons[index].emit_clicked();
                } else if editing
                    && let Some(button) = buttons.iter().find(|button| **button != self.back)
                {
                    button.emit_clicked();
                }
            }
            gdk::Key::Right
                if !editing && current.is_some_and(|index| buttons[index] == self.more_button) =>
            {
                self.change_page(true)
            }
            gdk::Key::Left if !editing && self.more.get() => self.change_page(false),
            gdk::Key::f | gdk::Key::F if modifiers.contains(gdk::ModifierType::CONTROL_MASK) => {
                self.search.grab_focus();
                self.search.select_region(0, -1);
            }
            gdk::Key::BackSpace if !editing && !self.search.text().is_empty() => {
                self.search.grab_focus();
                self.search.set_position(-1);
                super::super::shortcuts::palette_delete_backward(&self.search);
            }
            _ if !editing
                && !modifiers.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK
                        | gdk::ModifierType::META_MASK,
                ) =>
            {
                let Some(character) = key
                    .to_unicode()
                    .filter(|character| !character.is_control() && *character != ' ')
                else {
                    return glib::Propagation::Proceed;
                };
                self.search.grab_focus();
                self.search.set_position(-1);
                super::super::shortcuts::palette_insert_character(&self.search, character);
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }
}
