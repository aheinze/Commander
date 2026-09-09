//! The file view: list, grid, and Miller-column widgets, their factories, and drag and drop.

use super::context_menu::{CONTEXT_SPECIAL_KINDS, context_target_at};
use super::shortcuts::widget_has_ancestor_css_class;
use super::*;

pub(super) fn file_list_provider(paths: &[VPath]) -> Option<gdk::ContentProvider> {
    if paths.is_empty() {
        return None;
    }
    let files = paths
        .iter()
        .map(|path| gio::File::for_path(path.as_path()))
        .collect::<Vec<_>>();
    let files = gdk::FileList::from_array(&files);
    Some(gdk::ContentProvider::for_value(&files.to_value()))
}

pub(super) fn selected_drag_paths(state: &PaneState) -> Vec<VPath> {
    if state.selection.is_empty() {
        return Vec::new();
    }
    if state.view_mode == PaneViewMode::Columns {
        return state.selected_miller_sources();
    }
    let Some(listing) = state.active().listing.as_ref() else {
        return Vec::new();
    };
    listing
        .rows()
        .filter(|entry| {
            state
                .selection
                .contains(&SelectionKey::for_entry(listing.parent(), entry))
        })
        .map(|entry| listing.parent().join_name(entry.name()))
        .collect()
}

pub(super) fn paths_from_file_list(value: &glib::Value) -> Option<Vec<VPath>> {
    let files = value.get::<gdk::FileList>().ok()?;
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for file in files.files() {
        let path = VPath::from(file.path()?);
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    (!paths.is_empty()).then_some(paths)
}

pub(super) fn preferred_file_drop_action(
    target: &gtk::DropTarget,
    state: &FileDragUiState,
    destination: &VPath,
) -> gdk::DragAction {
    if !(action_policy::Context {
        history_busy: state.history_busy.get(),
        ..action_policy::Context::default()
    })
    .action(CommandId::Paste)
    .enabled()
    {
        return gdk::DragAction::empty();
    }
    let offered = target
        .current_drop()
        .map_or(gdk::DragAction::COPY | gdk::DragAction::MOVE, |drop| {
            drop.actions()
        });
    let access = state
        .archive_roots
        .borrow()
        .iter()
        .rev()
        .find(|(root, _)| destination.as_path().starts_with(root))
        .map(|(_, writable)| *writable);
    archive_file_drop_action(
        offered,
        target.current_event_state(),
        state.internal_active.get(),
        access,
    )
}

fn archive_file_drop_action(
    offered: gdk::DragAction,
    modifiers: gdk::ModifierType,
    internal: bool,
    archive: Option<bool>,
) -> gdk::DragAction {
    let destination = action_policy::Location::from_archive(archive);
    let desired = if destination.is_archive() && !modifiers.contains(gdk::ModifierType::SHIFT_MASK)
    {
        gdk::DragAction::COPY
    } else {
        choose_file_drop_action(offered, modifiers, internal)
    };
    if offered.contains(desired)
        && action_policy::transfer(
            action_policy::Location::Folder,
            destination,
            desired == gdk::DragAction::MOVE,
        )
        .is_ok()
    {
        desired
    } else {
        gdk::DragAction::empty()
    }
}

pub(super) fn choose_file_drop_action(
    offered: gdk::DragAction,
    modifiers: gdk::ModifierType,
    internal_drag: bool,
) -> gdk::DragAction {
    let preferred = if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        gdk::DragAction::COPY
    } else if modifiers.contains(gdk::ModifierType::SHIFT_MASK) || internal_drag {
        gdk::DragAction::MOVE
    } else {
        gdk::DragAction::COPY
    };
    if offered.contains(preferred) {
        preferred
    } else if offered.contains(gdk::DragAction::COPY) {
        gdk::DragAction::COPY
    } else if offered.contains(gdk::DragAction::MOVE) {
        gdk::DragAction::MOVE
    } else {
        gdk::DragAction::empty()
    }
}

pub(super) fn install_file_drag_source(
    widget: &impl IsA<gtk::Widget>,
    pane_drag: Rc<RefCell<PaneDragState>>,
    drag_ui: Rc<FileDragUiState>,
) {
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    {
        let pane_drag = Rc::clone(&pane_drag);
        source.connect_prepare(move |source, _, _| {
            let widget = source.widget()?;
            let state = pane_drag.borrow();
            source.set_actions(if state.archive_access.is_some() {
                gdk::DragAction::COPY
            } else {
                gdk::DragAction::COPY | gdk::DragAction::MOVE
            });
            file_list_provider(&state.drag_paths(&widget))
        });
    }
    {
        let drag_ui = Rc::clone(&drag_ui);
        source.connect_drag_begin(move |source, _| {
            drag_ui.internal_active.set(true);
            if let Some(widget) = source.widget() {
                widget.add_css_class("file-drag-source");
                let paintable = gtk::WidgetPaintable::new(Some(&widget));
                source.set_icon(Some(&paintable), 16, 16);
            }
        });
    }
    source.connect_drag_end(move |source, _, _| {
        drag_ui.internal_active.set(false);
        if let Some(widget) = source.widget() {
            widget.remove_css_class("file-drag-source");
        }
    });
    widget.as_ref().add_controller(source);
}

pub(super) fn install_file_drop_target(
    widget: &impl IsA<gtk::Widget>,
    sender: &ComponentSender<AppModel>,
    drag_ui: Rc<FileDragUiState>,
    destination: impl Fn(&gtk::Widget) -> Option<VPath> + 'static,
) {
    let destination: Rc<FileDropDestination> = Rc::new(destination);
    let target = gtk::DropTarget::new(
        gdk::FileList::static_type(),
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    {
        let destination = Rc::clone(&destination);
        let drag_ui = Rc::clone(&drag_ui);
        target.connect_enter(move |target, _, _| {
            let Some(widget) = target.widget() else {
                return gdk::DragAction::empty();
            };
            let Some(destination) = destination(&widget) else {
                return gdk::DragAction::empty();
            };
            widget.add_css_class("file-drop-target");
            preferred_file_drop_action(target, &drag_ui, &destination)
        });
    }
    {
        let destination = Rc::clone(&destination);
        let drag_ui = Rc::clone(&drag_ui);
        target.connect_motion(move |target, _, _| {
            let Some(widget) = target.widget() else {
                return gdk::DragAction::empty();
            };
            let Some(destination) = destination(&widget) else {
                return gdk::DragAction::empty();
            };
            preferred_file_drop_action(target, &drag_ui, &destination)
        });
    }
    target.connect_leave(|target| {
        if let Some(widget) = target.widget() {
            widget.remove_css_class("file-drop-target");
        }
    });
    {
        let input = sender.input_sender().clone();
        target.connect_drop(move |target, value, _, _| {
            let Some(widget) = target.widget() else {
                return false;
            };
            widget.remove_css_class("file-drop-target");
            let Some(destination) = destination(&widget) else {
                return false;
            };
            let Some(sources) = paths_from_file_list(value) else {
                return false;
            };
            let action = match preferred_file_drop_action(target, &drag_ui, &destination) {
                action if action.contains(gdk::DragAction::MOVE) => FileDropAction::Move,
                action if action.contains(gdk::DragAction::COPY) => FileDropAction::Copy,
                _ => return false,
            };
            input
                .send(AppMsg::DropFiles {
                    sources,
                    destination,
                    action,
                })
                .is_ok()
        });
    }
    widget.as_ref().add_controller(target);
}

pub(super) struct PaneWidgets {
    pub(super) pane: PaneId,
    pub(super) keymap: Keymap,
    pub(super) root: gtk::Box,
    pub(super) tab_bar: gtk::Box,
    pub(super) breadcrumb_stack: gtk::Stack,
    pub(super) breadcrumb_box: gtk::Box,
    pub(super) breadcrumb_overflow: gtk::MenuButton,
    pub(super) path_entry: gtk::Entry,
    location_display: String,
    location_focus: gtk::EventControllerFocus,
    pub(super) sort_dropdown: gtk::DropDown,
    pub(super) sort_direction: gtk::Button,
    pub(super) glob_revealer: gtk::Revealer,
    pub(super) glob_label: gtk::Label,
    pub(super) glob_entry: gtk::Entry,
    pub(super) model: ListingListModel,
    pub(super) view_stack: gtk::Stack,
    pub(super) loading_revealer: gtk::Revealer,
    pub(super) loading_label: gtk::Label,
    pub(super) column_view: gtk::ColumnView,
    pub(super) list_hadjustment: gtk::Adjustment,
    list_vadjustment: gtk::Adjustment,
    grid_vadjustment: gtk::Adjustment,
    scroll_path: Rc<RefCell<Option<VPath>>>,
    restoring_scroll: Rc<Cell<bool>>,
    rendered_scroll_restore: u64,
    pub(super) grid_view: gtk::GridView,
    pub(super) miller_box: gtk::Box,
    pub(super) miller_hadjustment: gtk::Adjustment,
    /// Set when a column was appended; consumed by the first scroll pass after layout.
    pub(super) miller_reveal_pending: Rc<Cell<bool>>,
    pub(super) miller_columns: Vec<MillerColumnWidgets>,
    pub(super) status: gtk::Label,
    pub(super) git: pane_git::GitStatusWidgets,
    pub(super) spinner: gtk::Spinner,
    pub(super) rendered_revision: u64,
    pub(super) rendered_metadata_revision: u64,
    pub(super) rendered_selection_revision: u64,
    pub(super) rendered_tabs_revision: u64,
    pub(super) rendered_cursor: Option<(u32, bool)>,
    pub(super) rendered_focus_files_epoch: u64,
    rendered_reveal_epoch: u64,
    pub(super) rendered_view_mode: Option<PaneViewMode>,
    pub(super) rendered_miller_revision: u64,
    pub(super) rendered_miller_columns: usize,
    pub(super) rendered_miller_cursor: Option<(VPath, Option<u32>)>,
    pub(super) rendered_sort: Option<SortSpec>,
    pub(super) rendered_glob: Option<(bool, bool, String)>,
    pub(super) rendered_active: Option<bool>,
    pub(super) rendered_status: String,
    pub(super) rendered_spinner: Option<bool>,
    pub(super) rendered_loading_overlay: Option<(bool, String)>,
    pub(super) paint_pending: Rc<Cell<bool>>,
    pub(super) pending_complete: Rc<Cell<bool>>,
    pub(super) bound_rows: Rc<Cell<u64>>,
    pub(super) max_bind_ns: Rc<Cell<u64>>,
    pub(super) max_render_ns: Rc<Cell<u64>>,
    pub(super) metadata_request_pending: Rc<Cell<bool>>,
    pub(super) tag_store: Rc<RefCell<BTreeMap<String, String>>>,
    pub(super) custom_tool_store: Rc<RefCell<Vec<CustomToolSession>>>,
    pub(super) thumbnail_generation: Rc<Cell<u64>>,
    pub(super) thumbnail_ui: Rc<RefCell<ThumbnailUiState>>,
    pub(super) pane_drag: Rc<RefCell<PaneDragState>>,
    pub(super) file_drag_ui: Rc<FileDragUiState>,
    pub(super) rendered_tags_revision: u64,
    pub(super) rendered_breadcrumb_path: Option<VPath>,
}

pub(super) struct MillerColumnWidgets {
    pub(super) count: gtk::Label,
    pub(super) placeholder: gtk::Box,
    pub(super) scroll: Option<gtk::ScrolledWindow>,
    pub(super) path: VPath,
    pub(super) container: gtk::Box,
    pub(super) model: Option<ListingListModel>,
    pub(super) view: Option<gtk::ListView>,
    rows: Rc<MillerRows>,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
}

#[derive(Default)]
pub(super) struct MillerRows {
    navigation_row: Cell<Option<u32>>,
    items: RefCell<Vec<glib::WeakRef<gtk::ListItem>>>,
}

impl MillerRows {
    fn style_item(&self, item: &gtk::ListItem) {
        if let Some(child) = item.child() {
            if self.navigation_row.get() == Some(item.position()) {
                child.add_css_class("miller-path");
            } else {
                child.remove_css_class("miller-path");
            }
        }
    }

    fn set_navigation_row(&self, row: Option<u32>) {
        if self.navigation_row.replace(row) != row {
            self.items.borrow_mut().retain(|weak| {
                let Some(item) = weak.upgrade() else {
                    return false;
                };
                self.style_item(&item);
                true
            });
        }
    }

    fn focused_row(&self) -> Option<u32> {
        self.items.borrow().iter().find_map(|weak| {
            let item = weak.upgrade()?;
            let row = item.child()?.parent()?;
            row.is_focus().then_some(item.position())
        })
    }
}

fn install_miller_resize_handle(
    container: &gtk::Box,
    pane: PaneId,
    path: &VPath,
    input: &relm4::Sender<AppMsg>,
) {
    let handle = gtk::Box::new(gtk::Orientation::Vertical, 0);
    handle.add_css_class("miller-resize-handle");
    handle.set_cursor_from_name(Some("col-resize"));
    handle.set_tooltip_text(Some("Drag to resize column · Double-click to reset"));
    let initial = Rc::new(Cell::new(280));
    let drag = gtk::GestureDrag::new();
    let weak = container.downgrade();
    let start = Rc::clone(&initial);
    drag.connect_drag_begin(move |_, _, _| {
        if let Some(container) = weak.upgrade() {
            start.set(container.width());
        }
    });
    let weak = container.downgrade();
    let start = Rc::clone(&initial);
    drag.connect_drag_update(move |_, offset, _| {
        if let Some(container) = weak.upgrade() {
            container.set_width_request(miller_column_width(start.get(), offset));
        }
    });
    let input_end = input.clone();
    let path_end = path.clone();
    drag.connect_drag_end(move |_, offset, _| {
        if offset.abs() >= 1.0 {
            let _ = input_end.send(AppMsg::MillerResize(
                pane,
                path_end.clone(),
                miller_column_width(initial.get(), offset),
            ));
        }
    });
    handle.add_controller(drag);
    let reset = gtk::GestureClick::new();
    reset.set_button(1);
    let input = input.clone();
    let path = path.clone();
    reset.connect_pressed(move |_, count, _, _| {
        if count == 2 {
            let _ = input.send(AppMsg::MillerResize(pane, path.clone(), 280));
        }
    });
    handle.add_controller(reset);
    container.append(&handle);
}

pub(super) fn miller_column_width(initial: i32, offset: f64) -> i32 {
    if !offset.is_finite() {
        return initial.clamp(220, 600);
    }
    (f64::from(initial) + offset).clamp(220.0, 600.0) as i32
}

impl PaneWidgets {
    pub(super) fn new(
        pane: PaneId,
        sender: &ComponentSender<AppModel>,
        keymap: Keymap,
        tag_store: Rc<RefCell<BTreeMap<String, String>>>,
        custom_tool_store: Rc<RefCell<Vec<CustomToolSession>>>,
        thumbnail_ui: Rc<RefCell<ThumbnailUiState>>,
        file_drag_ui: Rc<FileDragUiState>,
    ) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.add_css_class("pane");
        let click = gtk::GestureClick::new();
        {
            let input = sender.input_sender().clone();
            click.connect_pressed(move |_, _, _, _| {
                let _ = input.send(AppMsg::ActivatePane(pane));
            });
        }
        root.add_controller(click);
        let pane_drag = Rc::new(RefCell::new(PaneDragState::new()));
        let root_drag = Rc::clone(&pane_drag);
        install_file_drop_target(&root, sender, Rc::clone(&file_drag_ui), move |_| {
            root_drag.borrow().current_directory.clone()
        });

        let tab_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tab_bar.add_css_class("pane-tabs");
        tab_bar.set_hexpand(true);
        root.append(&tab_bar);

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        controls.add_css_class("pane-toolbar");
        let up = icon_button("commander-arrow-up-symbolic", "Parent directory");
        up.add_css_class("flat");
        up.add_css_class("breadcrumb-icon");
        let path_entry = gtk::Entry::new();
        path_entry.update_property(&[gtk::accessible::Property::Label("Folder location")]);
        path_entry.set_hexpand(true);
        path_entry.set_width_chars(1);
        path_entry.set_max_width_chars(1);
        path_entry.add_css_class("path-entry");
        path_entry.set_icon_from_icon_name(
            gtk::EntryIconPosition::Primary,
            Some("commander-folder-symbolic"),
        );
        path_entry.set_placeholder_text(Some("Location"));
        let breadcrumb_box = gtk::Box::new(gtk::Orientation::Horizontal, 1);
        breadcrumb_box.add_css_class("breadcrumbs");
        breadcrumb_box.set_hexpand(true);
        let breadcrumb_scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(1)
            .propagate_natural_width(false)
            .hexpand(true)
            .child(&breadcrumb_box)
            .build();
        breadcrumb_scrolled
            .hadjustment()
            .connect_changed(|adjustment| {
                // Finish the viewport allocation before scrolling to the current folder.
                // Updating synchronously can leave its child at the previous offset.
                let adjustment = adjustment.downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(adjustment) = adjustment.upgrade() {
                        adjustment
                            .set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
                    }
                });
            });
        let breadcrumb_overflow = gtk::MenuButton::new();
        breadcrumb_overflow.add_css_class("flat");
        breadcrumb_overflow.add_css_class("breadcrumb-icon");
        breadcrumb_overflow.set_icon_name("commander-ellipsis-symbolic");
        breadcrumb_overflow.set_tooltip_text(Some("Parent folders"));
        breadcrumb_overflow.update_property(&[gtk::accessible::Property::Label("Parent folders")]);
        let breadcrumb_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        breadcrumb_row.append(&breadcrumb_overflow);
        breadcrumb_row.append(&breadcrumb_scrolled);
        let breadcrumb_stack = gtk::Stack::new();
        breadcrumb_stack.set_hexpand(true);
        breadcrumb_stack.set_hhomogeneous(false);
        breadcrumb_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        breadcrumb_stack.set_transition_duration(100);
        breadcrumb_stack.add_named(&breadcrumb_row, Some("breadcrumbs"));
        breadcrumb_stack.add_named(&path_entry, Some("location"));
        breadcrumb_stack.set_visible_child_name("breadcrumbs");
        let sort_dropdown = gtk::DropDown::from_strings(&["Name", "Size", "Modified", "Type"]);
        sort_dropdown.update_property(&[gtk::accessible::Property::Label("Sort files by")]);
        sort_dropdown.add_css_class("sort-dropdown");
        sort_dropdown.set_tooltip_text(Some("Sort field"));
        sort_dropdown.set_valign(gtk::Align::Center);
        let sort_direction = icon_button("commander-arrow-down-a-z-symbolic", "Reverse sort order");
        sort_direction.add_css_class("flat");
        sort_direction.add_css_class("sort-direction");
        let sort_control = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        sort_control.add_css_class("sort-control");
        sort_control.set_valign(gtk::Align::Center);
        sort_control.append(&sort_dropdown);
        sort_control.append(&sort_direction);
        controls.append(&up);
        controls.append(&breadcrumb_stack);
        controls.append(&sort_control);
        let controls_scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(1)
            .propagate_natural_width(false)
            .hexpand(true)
            .child(&controls)
            .build();
        controls_scrolled
            .hadjustment()
            .connect_changed(|adjustment| {
                adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
            });
        root.append(&controls_scrolled);

        let glob_revealer = gtk::Revealer::new();
        glob_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        let glob_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        glob_box.add_css_class("glob-bar");
        let glob_label = gtk::Label::new(Some("Select glob"));
        let glob_entry = gtk::Entry::new();
        glob_entry.update_property(&[gtk::accessible::Property::Label("Selection glob pattern")]);
        glob_entry.set_hexpand(true);
        glob_entry.set_placeholder_text(Some("*.rs"));
        glob_box.append(&glob_label);
        glob_box.append(&glob_entry);
        glob_revealer.set_child(Some(&glob_box));
        root.append(&glob_revealer);

        connect_button(&up, sender, move || AppMsg::Up(pane));
        {
            let input = sender.input_sender().clone();
            let location = Rc::clone(&pane_drag);
            path_entry.connect_activate(move |entry| {
                let text = entry.text();
                // Keep native bytes when the displayed location was submitted unchanged.
                let path = location
                    .borrow()
                    .current_directory
                    .as_ref()
                    .filter(|path| path.to_string() == text)
                    .cloned()
                    .unwrap_or_else(|| VPath::from(text.as_str()));
                let _ = input.send(AppMsg::NavigateExact(pane, path));
            });
        }
        let location_focus = gtk::EventControllerFocus::new();
        {
            let stack = breadcrumb_stack.downgrade();
            location_focus.connect_leave(move |_| {
                if let Some(stack) = stack.upgrade() {
                    stack.set_visible_child_name("breadcrumbs");
                }
            });
        }
        path_entry.add_controller(location_focus.clone());
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let input = sender.input_sender().clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                let _ = input.send(AppMsg::CancelLocation(pane));
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        path_entry.add_controller(keys);
        {
            let input = sender.input_sender().clone();
            sort_dropdown.connect_selected_notify(move |dropdown| {
                let key = match dropdown.selected() {
                    1 => SortKey::Size,
                    2 => SortKey::Modified,
                    3 => SortKey::Kind,
                    _ => SortKey::Name,
                };
                let _ = input.send(AppMsg::SetSort(pane, key));
            });
        }
        connect_button(&sort_direction, sender, move || {
            AppMsg::ToggleSortDirection(pane)
        });
        {
            let input = sender.input_sender().clone();
            glob_entry.connect_changed(move |entry| {
                let _ = input.send(AppMsg::SetGlob(pane, entry.text().to_string()));
            });
        }
        {
            let input = sender.input_sender().clone();
            glob_entry.connect_activate(move |_| {
                let _ = input.send(AppMsg::ApplyGlob(pane));
            });
        }

        let model = ListingListModel::new();
        {
            let input = sender.input_sender().clone();
            model.set_selection_callback(move |selection, cursor_row| {
                let _ = input.send(AppMsg::SelectionChanged(pane, selection, cursor_row));
            });
        }
        let column_view = gtk::ColumnView::new(Some(model.clone()));
        column_view.update_property(&[
            gtk::accessible::Property::Label("Files in this pane"),
            gtk::accessible::Property::MultiSelectable(true),
        ]);
        column_view.set_hexpand(true);
        column_view.set_vexpand(true);
        column_view.add_css_class("file-list");
        column_view.set_enable_rubberband(true);
        column_view.set_show_column_separators(false);
        column_view.set_show_row_separators(false);

        let bound_rows = Rc::new(Cell::new(0));
        let max_bind_ns = Rc::new(Cell::new(0));
        let max_render_ns = Rc::new(Cell::new(0));
        let metadata_request_pending = Rc::new(Cell::new(false));
        append_column(
            &column_view,
            pane,
            ColumnKind::Name,
            sender,
            Rc::clone(&bound_rows),
            Rc::clone(&max_bind_ns),
            Rc::clone(&metadata_request_pending),
            Rc::clone(&tag_store),
            Rc::clone(&pane_drag),
            Rc::clone(&file_drag_ui),
        );
        for kind in [ColumnKind::Size, ColumnKind::Modified] {
            append_column(
                &column_view,
                pane,
                kind,
                sender,
                Rc::clone(&bound_rows),
                Rc::clone(&max_bind_ns),
                Rc::clone(&metadata_request_pending),
                Rc::clone(&tag_store),
                Rc::clone(&pane_drag),
                Rc::clone(&file_drag_ui),
            );
        }
        {
            let input = sender.input_sender().clone();
            column_view.connect_activate(move |_, position| {
                let _ = input.send(AppMsg::OpenRow(pane, position));
            });
        }
        install_file_context_menu(
            &column_view,
            pane,
            sender,
            keymap.clone(),
            Rc::clone(&custom_tool_store),
            Rc::clone(&pane_drag),
        );

        let list_scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&column_view)
            .build();
        let list_hadjustment = list_scrolled.hadjustment();
        let list_vadjustment = list_scrolled.vadjustment();
        let scroll_path = Rc::new(RefCell::new(None));
        let restoring_scroll = Rc::new(Cell::new(false));
        observe_scroll(
            &list_scrolled,
            &list_vadjustment,
            pane,
            false,
            scroll_path.clone(),
            restoring_scroll.clone(),
            sender.input_sender(),
        );
        list_hadjustment.connect_value_changed(|adjustment| {
            let leading_edge = adjustment.lower();
            if (adjustment.value() - leading_edge).abs() > f64::EPSILON {
                adjustment.set_value(leading_edge);
            }
        });
        list_scrolled.add_css_class("file-view-scroller");

        let thumbnail_generation = Rc::new(Cell::new(0));
        let grid_factory = build_grid_factory(
            pane,
            Rc::clone(&thumbnail_generation),
            Rc::clone(&thumbnail_ui),
            Rc::clone(&pane_drag),
            Rc::clone(&file_drag_ui),
            sender,
            Rc::clone(&tag_store),
        );
        let grid_view = gtk::GridView::new(Some(model.clone()), Some(grid_factory));
        grid_view.update_property(&[
            gtk::accessible::Property::Label("Files in this pane as icons"),
            gtk::accessible::Property::MultiSelectable(true),
        ]);
        grid_view.add_css_class("file-grid");
        grid_view.set_hexpand(true);
        grid_view.set_vexpand(true);
        grid_view.set_enable_rubberband(true);
        grid_view.set_min_columns(2);
        grid_view.set_max_columns(12);
        {
            let input = sender.input_sender().clone();
            grid_view.connect_activate(move |_, position| {
                let _ = input.send(AppMsg::OpenRow(pane, position));
            });
        }
        install_file_context_menu(
            &grid_view,
            pane,
            sender,
            keymap.clone(),
            Rc::clone(&custom_tool_store),
            Rc::clone(&pane_drag),
        );
        let grid_scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&grid_view)
            .build();
        grid_scrolled.add_css_class("file-view-scroller");
        let grid_vadjustment = grid_scrolled.vadjustment();
        observe_scroll(
            &grid_scrolled,
            &grid_vadjustment,
            pane,
            false,
            scroll_path.clone(),
            restoring_scroll.clone(),
            sender.input_sender(),
        );

        let miller_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        miller_box.add_css_class("miller-browser");
        miller_box.set_vexpand(true);
        let column_scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .child(&miller_box)
            .build();
        column_scrolled.add_css_class("file-view-scroller");
        let miller_hadjustment = column_scrolled.hadjustment();
        observe_scroll(
            &column_scrolled,
            &miller_hadjustment,
            pane,
            true,
            scroll_path.clone(),
            restoring_scroll.clone(),
            sender.input_sender(),
        );

        let view_stack = gtk::Stack::new();
        view_stack.set_hexpand(true);
        view_stack.set_vexpand(true);
        view_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        view_stack.set_transition_duration(120);
        view_stack.add_named(&list_scrolled, Some("list"));
        view_stack.add_named(&grid_scrolled, Some("grid"));
        view_stack.add_named(&column_scrolled, Some("columns"));

        let loading_revealer = gtk::Revealer::new();
        loading_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        loading_revealer.set_transition_duration(120);
        loading_revealer.set_halign(gtk::Align::Center);
        loading_revealer.set_valign(gtk::Align::Start);
        loading_revealer.set_margin_top(14);
        loading_revealer.set_can_target(false);
        let loading_card = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        loading_card.add_css_class("files-loading");
        let loading_spinner = gtk::Spinner::new();
        loading_spinner.set_spinning(true);
        loading_spinner.set_size_request(14, 14);
        let loading_label = gtk::Label::new(Some("Loading items…"));
        loading_label.add_css_class("files-loading-label");
        loading_card.append(&loading_spinner);
        loading_card.append(&loading_label);
        loading_revealer.set_child(Some(&loading_card));

        let view_overlay = gtk::Overlay::new();
        view_overlay.set_hexpand(true);
        view_overlay.set_vexpand(true);
        view_overlay.set_child(Some(&view_stack));
        view_overlay.add_overlay(&loading_revealer);
        root.append(&view_overlay);

        let status_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        status_box.add_css_class("pane-status");
        let spinner = gtk::Spinner::new();
        let status = gtk::Label::new(Some("Loading…"));
        status.set_xalign(0.0);
        status.set_hexpand(true);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_width_chars(1);
        status_box.append(&spinner);
        status_box.append(&status);
        let git = pane_git::GitStatusWidgets::new();
        status_box.append(&git.root);
        let terminal = icon_button(
            "commander-terminal-symbolic",
            "Open this folder in a new terminal",
        );
        terminal.add_css_class("pane-status-action");
        terminal.set_valign(gtk::Align::Center);
        {
            let input = sender.input_sender().clone();
            terminal.connect_clicked(move |_| {
                let _ = input.send(AppMsg::ActivatePane(pane));
                let _ = input.send(AppMsg::NewTerminal);
            });
        }
        status_box.append(&terminal);
        root.append(&status_box);

        Self {
            pane,
            keymap,
            root,
            tab_bar,
            breadcrumb_stack,
            breadcrumb_box,
            breadcrumb_overflow,
            path_entry,
            location_display: String::new(),
            location_focus,
            sort_dropdown,
            sort_direction,
            glob_revealer,
            glob_label,
            glob_entry,
            model,
            view_stack,
            loading_revealer,
            loading_label,
            column_view,
            list_hadjustment,
            list_vadjustment,
            grid_vadjustment,
            scroll_path,
            restoring_scroll,
            rendered_scroll_restore: u64::MAX,
            grid_view,
            miller_box,
            miller_hadjustment,
            miller_reveal_pending: Rc::default(),
            miller_columns: Vec::new(),
            status,
            git,
            spinner,
            rendered_revision: u64::MAX,
            rendered_metadata_revision: u64::MAX,
            rendered_selection_revision: u64::MAX,
            rendered_tabs_revision: u64::MAX,
            rendered_cursor: None,
            rendered_focus_files_epoch: 0,
            rendered_reveal_epoch: 0,
            rendered_view_mode: None,
            rendered_miller_revision: u64::MAX,
            rendered_miller_columns: 0,
            rendered_miller_cursor: None,
            rendered_sort: None,
            rendered_glob: None,
            rendered_active: None,
            rendered_status: String::new(),
            rendered_spinner: None,
            rendered_loading_overlay: None,
            paint_pending: Rc::new(Cell::new(false)),
            pending_complete: Rc::new(Cell::new(false)),
            bound_rows,
            max_bind_ns,
            max_render_ns,
            metadata_request_pending,
            tag_store,
            custom_tool_store,
            thumbnail_generation,
            thumbnail_ui,
            pane_drag,
            file_drag_ui,
            rendered_tags_revision: u64::MAX,
            rendered_breadcrumb_path: None,
        }
    }

    pub(super) fn focus_location(&self) {
        self.path_entry.set_text(&self.location_display);
        self.breadcrumb_stack.set_visible_child_name("location");
        self.path_entry.grab_focus();
        self.path_entry.select_region(0, -1);
    }

    pub(super) fn focus_files(&self, mode: PaneViewMode) {
        match mode {
            PaneViewMode::List => {
                self.column_view.grab_focus();
            }
            PaneViewMode::Grid => {
                self.grid_view.grab_focus();
            }
            PaneViewMode::Columns => {
                if let Some(view) = self
                    .miller_columns
                    .last()
                    .and_then(|column| column.view.as_ref())
                {
                    view.grab_focus();
                } else {
                    self.miller_box.grab_focus();
                }
            }
        }
    }

    pub(super) fn render(
        &mut self,
        state: &PaneState,
        active: bool,
        started: Instant,
        tags_revision: u64,
        archives: &archive_browser::ArchiveLocations,
        sender: &ComponentSender<AppModel>,
    ) {
        let render_started = Instant::now();
        if self.rendered_scroll_restore != state.scroll_restore_epoch {
            self.restoring_scroll.set(true);
        }
        *self.scroll_path.borrow_mut() = Some(state.active().path.clone());
        *self.file_drag_ui.archive_roots.borrow_mut() = archives.drop_roots();
        self.pane_drag.borrow_mut().archive_access =
            archives.contains(state.current_directory()).then(|| {
                archives
                    .read_only_reason(state.current_directory())
                    .is_none()
            });
        let drag_state_changed = self.rendered_revision != state.revision
            || self.rendered_selection_revision != state.selection_revision
            || self.rendered_miller_revision != state.miller_revision
            || self.rendered_view_mode != Some(state.view_mode)
            || self.pane_drag.borrow().current_directory.as_ref()
                != Some(state.current_directory());
        if drag_state_changed {
            let mut pane_drag = self.pane_drag.borrow_mut();
            pane_drag.current_directory = Some(state.current_directory().clone());
            pane_drag.selected = selected_drag_paths(state);
        }
        let mut thumbnail_rebind = HashSet::new();
        if self.thumbnail_generation.get() != state.generation {
            self.thumbnail_generation.set(state.generation);
            thumbnail_rebind = self
                .thumbnail_ui
                .borrow_mut()
                .begin_generation(self.pane, state.generation);
        }
        if self.rendered_active != Some(active) {
            self.rendered_active = Some(active);
            if active {
                self.root.remove_css_class("pane-inactive");
                self.root.add_css_class("pane-active");
            } else {
                self.root.remove_css_class("pane-active");
                self.root.add_css_class("pane-inactive");
            }
        }
        let path = archives.display(state.current_directory()).to_string();
        self.location_display.clone_from(&path);
        if !self.location_focus.contains_focus() && self.path_entry.text().as_str() != path {
            self.path_entry.set_text(&path);
            self.path_entry.set_position(0);
        }
        if self.rendered_breadcrumb_path.as_ref() != Some(state.current_directory())
            || self.rendered_tags_revision != tags_revision
        {
            self.rendered_breadcrumb_path = Some(state.current_directory().clone());
            while let Some(child) = self.breadcrumb_box.first_child() {
                self.breadcrumb_box.remove(&child);
            }
            let ancestors = archives.ancestors(state.current_directory());
            let parents = gtk::ListBox::new();
            parents.set_selection_mode(gtk::SelectionMode::Single);
            parents.set_activate_on_single_click(true);
            let destinations: Vec<_> = ancestors.iter().rev().skip(1).cloned().collect();
            for destination in &destinations {
                let display = archives.display(destination).to_string();
                let label = gtk::Label::new(Some(&display));
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_max_width_chars(36);
                label.set_margin_start(8);
                label.set_margin_end(8);
                label.set_margin_top(7);
                label.set_margin_bottom(7);
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&label));
                row.set_tooltip_text(Some(&display));
                row.update_property(&[gtk::accessible::Property::Label(&display)]);
                parents.append(&row);
            }
            let popover = gtk::Popover::new();
            popover.set_has_arrow(false);
            popover.add_css_class("breadcrumb-ancestors");
            let scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .overlay_scrolling(false)
                .propagate_natural_height(true)
                .propagate_natural_width(true)
                .min_content_width(280)
                .max_content_width(360)
                .max_content_height(320)
                .child(&parents)
                .build();
            popover.set_child(Some(&scroll));
            let input = sender.input_sender().clone();
            let pane = self.pane;
            let weak = popover.downgrade();
            parents.connect_row_activated(move |_, row| {
                if let Some(path) = destinations.get(row.index() as usize) {
                    let _ = input.send(AppMsg::NavigateExact(pane, path.clone()));
                }
                if let Some(popover) = weak.upgrade() {
                    popover.popdown();
                }
            });
            self.breadcrumb_overflow.set_visible(ancestors.len() > 1);
            self.breadcrumb_overflow.set_popover(Some(&popover));
            let visible_from = ancestors.len().saturating_sub(5);
            for (index, ancestor) in ancestors.into_iter().enumerate().skip(visible_from) {
                if index > visible_from {
                    let separator = gtk::Label::new(Some("›"));
                    separator.add_css_class("breadcrumb-separator");
                    self.breadcrumb_box.append(&separator);
                }
                let display = archives.display(&ancestor);
                let label = display
                    .file_name()
                    .map(display_name)
                    .unwrap_or_else(|| "/".to_owned());
                let content = gtk::Box::new(gtk::Orientation::Horizontal, 5);
                if archives.is_root(&ancestor) {
                    let icon = gtk::Image::from_icon_name("commander-archive-symbolic");
                    icon.set_pixel_size(13);
                    content.append(&icon);
                }
                if let Some(color) = self.tag_store.borrow().get(&ancestor.to_string()) {
                    let indicator = tag_indicator();
                    apply_tag_indicator(&indicator, Some(color));
                    content.append(&indicator);
                }
                let text = gtk::Label::new(Some(&label));
                text.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                text.set_max_width_chars(18);
                let minimum = if &ancestor == state.current_directory() {
                    12
                } else {
                    5
                };
                text.set_width_chars(label.chars().count().min(minimum) as i32);
                content.append(&text);
                let button = gtk::Button::new();
                button.add_css_class("flat");
                button.add_css_class("breadcrumb-button");
                button.set_child(Some(&content));
                button.set_tooltip_text(Some(&display.to_string()));
                button.update_property(&[gtk::accessible::Property::Label(&display.to_string())]);
                if &ancestor == state.current_directory() {
                    button.add_css_class("current-folder");
                }
                let destination = ancestor.clone();
                let drop_destination = ancestor.clone();
                install_file_drop_target(
                    &button,
                    sender,
                    Rc::clone(&self.file_drag_ui),
                    move |_| Some(drop_destination.clone()),
                );
                let input = sender.input_sender().clone();
                let pane = self.pane;
                button.connect_clicked(move |_| {
                    let _ = input.send(AppMsg::NavigateExact(pane, destination.clone()));
                });
                self.breadcrumb_box.append(&button);
            }
        }
        let opening_archive = state.archive_browse.source.as_ref();
        let spinning = state.loading || opening_archive.is_some();
        if self.rendered_spinner != Some(spinning) {
            self.rendered_spinner = Some(spinning);
            self.spinner.set_spinning(spinning);
        }
        let loading_text = state.active().listing.as_ref().map_or_else(
            || "Loading items…".to_owned(),
            |listing| {
                if listing.is_empty() {
                    "Loading items…".to_owned()
                } else {
                    format!("Loading… {} items", listing.len())
                }
            },
        );
        let loading_text = opening_archive.map_or(loading_text, |source| {
            format!(
                "Opening {}… · Esc to cancel",
                source
                    .file_name()
                    .map(display_name)
                    .unwrap_or_else(|| "archive".to_owned())
            )
        });
        let loading_overlay = (spinning, loading_text);
        if self.rendered_loading_overlay.as_ref() != Some(&loading_overlay) {
            self.loading_label.set_label(&loading_overlay.1);
            self.loading_revealer.set_reveal_child(loading_overlay.0);
            self.rendered_loading_overlay = Some(loading_overlay);
        }
        let miller_column = (state.view_mode == PaneViewMode::Columns)
            .then(|| state.miller_columns.last())
            .flatten();
        let status_listing = miller_column.map_or(state.active().listing.as_ref(), |column| {
            column.listing.as_ref()
        });
        let status_error = state
            .error
            .as_ref()
            .or_else(|| miller_column.and_then(|column| column.error.as_ref()));
        let mut status = if let Some(source) = opening_archive {
            format!(
                "Opening {}… · Esc to cancel",
                source
                    .file_name()
                    .map(display_name)
                    .unwrap_or_else(|| "archive".to_owned())
            )
        } else if let Some(listing) = status_listing {
            if listing.is_complete() {
                let item_count = if state.filter_query.is_empty()
                    || state.filtering
                    || (miller_column.is_some() && state.miller_columns.len() > 1)
                {
                    format!("{} items", listing.len())
                } else {
                    format!("{} of {} items", listing.len(), listing.source_len())
                };
                let mut status = format!("{item_count}, {} selected", state.selection.len());
                if !state.selection.is_empty() {
                    status.push_str(" · ");
                    status.push_str(&state.selection_size_label());
                }
                status
            } else {
                format!("Loading… {} items", listing.len())
            }
        } else if status_error.is_some() {
            String::new()
        } else {
            "Loading…".to_owned()
        };
        if archives.contains(state.current_directory()) && opening_archive.is_none() {
            status.push_str(
                if archives
                    .read_only_reason(state.current_directory())
                    .is_some()
                {
                    " · Archive · read-only"
                } else {
                    " · Archive"
                },
            );
        }
        if self.rendered_status != status {
            self.rendered_status.clone_from(&status);
            self.status.set_label(&status);
            let tooltip = archives
                .read_only_reason(state.current_directory())
                .map_or_else(|| status.clone(), |reason| format!("{status}\n{reason}"));
            self.status.set_tooltip_text(Some(&tooltip));
        }
        self.git.render(state.git.info.as_ref());
        if self.rendered_view_mode != Some(state.view_mode) {
            self.rendered_view_mode = Some(state.view_mode);
            self.view_stack
                .set_visible_child_name(state.view_mode.name());
        }
        let tags_changed = self.rendered_tags_revision != tags_revision;
        if tags_changed {
            self.rendered_tags_revision = tags_revision;
        }
        if self.rendered_miller_revision != state.miller_revision || tags_changed {
            self.rendered_miller_revision = state.miller_revision;
            self.render_miller(state, active, tags_changed, archives, sender);
        }
        if active && self.rendered_focus_files_epoch != state.focus_files_epoch {
            self.rendered_focus_files_epoch = state.focus_files_epoch;
            self.focus_files(state.view_mode);
        }
        if self.rendered_sort != Some(state.sort) {
            self.rendered_sort = Some(state.sort);
            self.sort_dropdown.set_selected(match state.sort.key {
                SortKey::Name => 0,
                SortKey::Size => 1,
                SortKey::Modified => 2,
                SortKey::Kind => 3,
            });
            self.sort_direction
                .set_icon_name(match state.sort.direction {
                    SortDirection::Ascending => "commander-arrow-down-a-z-symbolic",
                    SortDirection::Descending => "commander-arrow-down-z-a-symbolic",
                });
        }
        let glob_state = (state.glob_open, state.glob_select, state.glob_query.clone());
        if self.rendered_glob.as_ref() != Some(&glob_state) {
            self.rendered_glob = Some(glob_state);
            self.glob_revealer.set_reveal_child(state.glob_open);
            self.glob_label.set_label(if state.glob_select {
                "Select glob"
            } else {
                "Deselect glob"
            });
            self.glob_entry.set_tooltip_text(Some(if state.glob_select {
                "Add matching entries to the selection"
            } else {
                "Remove matching entries from the selection"
            }));
            if self.glob_entry.text().as_str() != state.glob_query {
                self.glob_entry.set_text(&state.glob_query);
            }
            if state.glob_open {
                self.glob_entry.grab_focus();
            }
        }

        if self.rendered_tabs_revision != state.tabs_revision || tags_changed {
            self.rendered_tabs_revision = state.tabs_revision;
            self.render_tabs(state, archives, sender);
        }
        if self.rendered_revision != state.revision {
            if self.rendered_scroll_restore != state.scroll_restore_epoch {
                self.restoring_scroll.set(true);
            }
            self.rendered_revision = state.revision;
            let listing = state.active().listing.clone();
            let complete = listing
                .as_ref()
                .is_some_and(|listing| listing.is_complete());
            let populated = listing.as_ref().is_some_and(|listing| !listing.is_empty());
            self.metadata_request_pending.set(false);
            let model_started = Instant::now();
            self.model.set_listing(listing);
            tracing::debug!(
                pane = self.pane.label(),
                elapsed_ms = model_started.elapsed().as_secs_f64() * 1_000.0,
                "listing model swapped"
            );
            self.rendered_metadata_revision = state.metadata_revision;
            self.rendered_cursor = None;
            if populated {
                self.schedule_paint(complete, started, sender);
            }
        }
        if tags_changed {
            self.model.refresh_visible();
        }
        if self.rendered_metadata_revision != state.metadata_revision {
            self.rendered_metadata_revision = state.metadata_revision;
            self.metadata_request_pending.set(false);
            if let Some(listing) = state.active().listing.clone() {
                self.model.update_metadata(listing);
            }
        }
        if self.rendered_selection_revision != state.selection_revision {
            self.rendered_selection_revision = state.selection_revision;
            self.model.set_stable_selection(&state.selection);
        }
        if !thumbnail_rebind.is_empty() {
            // Metadata-only updates preserve other rows; restart any thumbnail work
            // cancelled with the old generation without rebinding the whole grid.
            self.model.refresh_paths(&thumbnail_rebind);
        }
        let cursor_state = (state.cursor_row, active);
        if self.rendered_cursor != Some(cursor_state) {
            self.rendered_cursor = Some(cursor_state);
            if active
                && self.rendered_scroll_restore == state.scroll_restore_epoch
                && state
                    .active()
                    .listing
                    .as_ref()
                    .is_some_and(|listing| (state.cursor_row as usize) < listing.len())
            {
                match state.view_mode {
                    PaneViewMode::List => self.column_view.scroll_to(
                        state.cursor_row,
                        None,
                        gtk::ListScrollFlags::FOCUS,
                        None,
                    ),
                    PaneViewMode::Grid => self.grid_view.scroll_to(
                        state.cursor_row,
                        gtk::ListScrollFlags::FOCUS,
                        None,
                    ),
                    PaneViewMode::Columns => {
                        if let Some((view, row)) = self
                            .miller_columns
                            .last()
                            .and_then(|column| column.view.as_ref())
                            .zip(
                                state
                                    .miller_columns
                                    .last()
                                    .and_then(|column| column.selected_row),
                            )
                        {
                            view.scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
                        }
                    }
                }
            }
        }
        if self.rendered_scroll_restore != state.scroll_restore_epoch && !state.loading {
            self.rendered_scroll_restore = state.scroll_restore_epoch;
            let adjustment = match state.view_mode {
                PaneViewMode::List => self.list_vadjustment.clone(),
                PaneViewMode::Grid => self.grid_vadjustment.clone(),
                PaneViewMode::Columns => self.miller_hadjustment.clone(),
            };
            let value = if state.view_mode == PaneViewMode::Columns {
                state.miller_scroll_x
            } else {
                state.scroll_y
            };
            let guard = self.restoring_scroll.clone();
            guard.set(true);
            glib::timeout_add_local_once(Duration::from_millis(40), move || {
                adjustment.set_value(
                    (value as f64).min((adjustment.upper() - adjustment.page_size()).max(0.0)),
                );
                guard.set(false);
            });
        }
        if active && self.rendered_reveal_epoch != state.reveal_epoch {
            self.rendered_reveal_epoch = state.reveal_epoch;
            self.reveal_cursor(state);
        }
        let elapsed = u64::try_from(render_started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.max_render_ns
            .set(self.max_render_ns.get().max(elapsed));
    }

    fn reveal_cursor(&self, state: &PaneState) {
        let row = state.cursor_row;
        let (model, view): (_, gtk::Widget) = match state.view_mode {
            PaneViewMode::List => (self.model.clone(), self.column_view.clone().upcast()),
            PaneViewMode::Grid => (self.model.clone(), self.grid_view.clone().upcast()),
            PaneViewMode::Columns => {
                let Some(column) = self.miller_columns.last() else {
                    return;
                };
                let Some((model, view)) = column.model.as_ref().zip(column.view.as_ref()) else {
                    return;
                };
                (model.clone(), view.clone().upcast())
            }
        };
        let Some(listing) = model.listing() else {
            return;
        };
        let Some(entry) = listing.row(row as usize) else {
            return;
        };
        let target = listing.parent().join_name(entry.name());
        let root = self.root.downgrade();
        let view = view.downgrade();
        // Restore the folder scroll first, then wait for the destination view's
        // first allocation. A new Miller column can still be 0 × 0 at this point.
        glib::timeout_add_local_once(Duration::from_millis(60), move || {
            let Some(view) = view.upgrade() else {
                return;
            };
            view.add_tick_callback(move |view, _| {
                let Some(root) = root
                    .upgrade()
                    .filter(|root| root.has_css_class("pane-active"))
                else {
                    return glib::ControlFlow::Break;
                };
                let still_selected = model.is_selected(row)
                    && model.listing().is_some_and(|listing| {
                        listing
                            .row(row as usize)
                            .is_some_and(|entry| listing.parent().join_name(entry.name()) == target)
                    });
                if !still_selected || !view.is_visible() || !root.is_visible() {
                    return glib::ControlFlow::Break;
                }
                if view.width() == 0 || view.height() == 0 {
                    return glib::ControlFlow::Continue;
                }
                if let Some(view) = view.downcast_ref::<gtk::ColumnView>() {
                    view.scroll_to(row, None, gtk::ListScrollFlags::FOCUS, None);
                } else if let Some(view) = view.downcast_ref::<gtk::GridView>() {
                    view.scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
                } else if let Some(view) = view.downcast_ref::<gtk::ListView>() {
                    view.scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
                }
                glib::ControlFlow::Break
            });
        });
    }

    pub(super) fn render_miller(
        &mut self,
        state: &PaneState,
        active: bool,
        tags_changed: bool,
        archives: &archive_browser::ArchiveLocations,
        sender: &ComponentSender<AppModel>,
    ) {
        if state.miller_columns.len() > self.rendered_miller_columns {
            self.miller_reveal_pending.set(true);
        }
        self.rendered_miller_columns = state.miller_columns.len();
        let reusable = self
            .miller_columns
            .iter()
            .zip(&state.miller_columns)
            .take_while(|(widgets, column)| {
                widgets.path == column.path
                    && widgets.model.is_some() == column.listing.is_some()
                    && (widgets.model.is_some()
                        || (widgets.loading == column.loading && widgets.error == column.error))
            })
            .count();
        while self.miller_columns.len() > reusable {
            if let Some(column) = self.miller_columns.pop() {
                self.miller_box.remove(&column.container);
            }
        }
        for (column_index, column) in state
            .miller_columns
            .iter()
            .enumerate()
            .skip(self.miller_columns.len())
        {
            let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            container.add_css_class("miller-column");
            container.set_width_request(column.width);
            container.set_hexpand(false);
            container.set_vexpand(true);
            let drop_path = column.path.clone();
            install_file_drop_target(&container, sender, self.file_drag_ui.clone(), move |_| {
                Some(drop_path.clone())
            });
            let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
            body.set_hexpand(true);
            body.set_overflow(gtk::Overflow::Hidden);
            let heading = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            heading.add_css_class("miller-column-heading");
            heading.append(&gtk::Image::from_icon_name(
                if archives.is_root(&column.path) {
                    "commander-archive-symbolic"
                } else {
                    "commander-folder-symbolic"
                },
            ));
            let display = archives.display(&column.path);
            let title = gtk::Label::new(Some(
                &display
                    .file_name()
                    .map_or_else(|| "File System".to_owned(), display_name),
            ));
            title.set_xalign(0.0);
            title.set_hexpand(true);
            title.set_width_chars(1);
            title.set_max_width_chars(1);
            title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            heading.set_tooltip_text(Some(&display.to_string()));
            let count = gtk::Label::new(None);
            count.add_css_class("miller-column-count");
            heading.append(&title);
            heading.append(&count);
            body.append(&heading);
            let placeholder = gtk::Box::new(gtk::Orientation::Vertical, 8);
            placeholder.add_css_class("miller-placeholder");
            placeholder.set_vexpand(true);
            placeholder.set_valign(gtk::Align::Center);
            if column.loading {
                let spinner = gtk::Spinner::new();
                spinner.set_spinning(true);
                placeholder.append(&spinner);
            } else {
                let icon = gtk::Image::from_icon_name(if column.error.is_some() {
                    "commander-triangle-alert-symbolic"
                } else {
                    "commander-folder-open-symbolic"
                });
                icon.set_pixel_size(28);
                placeholder.append(&icon);
            }
            let message = gtk::Label::new(Some(if column.error.is_some() {
                ""
            } else if column.loading {
                "Loading…"
            } else {
                "This folder is empty"
            }));
            message.set_wrap(true);
            message.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            message.set_width_chars(1);
            message.set_justify(gtk::Justification::Center);
            placeholder.append(&message);
            if column.error.is_some() {
                let retry = gtk::Button::with_label("Try Again");
                retry.set_halign(gtk::Align::Center);
                let path = column.path.clone();
                let pane = self.pane;
                connect_button(&retry, sender, move || {
                    AppMsg::MillerRetry(pane, column_index, path.clone())
                });
                placeholder.append(&retry);
            }
            let rows = Rc::new(MillerRows::default());
            let (model, view, scroll) = if let Some(listing) = column.listing.clone() {
                let model = ListingListModel::new();
                model.set_listing(Some(listing));
                let input = sender.input_sender().clone();
                let pane = self.pane;
                let path = column.path.clone();
                let selection_rows = Rc::clone(&rows);
                model.set_selection_callback(move |selection, row| {
                    // GTK reports ranges in ascending order. The focused row
                    // identifies the clicked endpoint, including upward ranges.
                    let row = selection_rows.focused_row().or(row);
                    let _ = input.send(AppMsg::MillerSelectionChanged {
                        pane,
                        column: column_index,
                        path: path.clone(),
                        selection,
                        row,
                    });
                });
                let factory = build_column_browser_factory(
                    Rc::clone(&rows),
                    Rc::clone(&self.tag_store),
                    Rc::clone(&self.pane_drag),
                    Rc::clone(&self.file_drag_ui),
                    sender,
                );
                let view = gtk::ListView::new(Some(model.clone()), Some(factory));
                view.update_property(&[
                    gtk::accessible::Property::Label(&format!("Files in {}", column.path)),
                    gtk::accessible::Property::MultiSelectable(true),
                ]);
                view.add_css_class("column-browser");
                // GTK couples single-click activation to selection on hover.
                // Keep its normal selection behavior and activate folders only
                // after an explicit, unmodified click has been released.
                view.set_single_click_activate(false);
                view.set_hexpand(true);
                view.set_vexpand(true);
                let input = sender.input_sender().clone();
                view.connect_activate(move |_, position| {
                    let _ = input.send(AppMsg::MillerOpen(pane, column_index, position));
                });
                let folder_click = gtk::GestureClick::new();
                folder_click.set_name(Some("miller-folder-click"));
                folder_click.set_button(1);
                // Bubble after GTK's row selection so opening the folder is the
                // last action. A claimed drag cancels this click gesture.
                folder_click.set_propagation_phase(gtk::PropagationPhase::Bubble);
                let input = sender.input_sender().clone();
                let weak_view = view.downgrade();
                let click_model = model.clone();
                let pane_drag = Rc::clone(&self.pane_drag);
                folder_click.connect_released(move |gesture, count, x, y| {
                    if count != 1
                        || gesture.current_event_state().intersects(
                            gdk::ModifierType::CONTROL_MASK
                                | gdk::ModifierType::SHIFT_MASK
                                | gdk::ModifierType::ALT_MASK
                                | gdk::ModifierType::SUPER_MASK
                                | gdk::ModifierType::META_MASK,
                        )
                    {
                        return;
                    }
                    let Some(view) = weak_view.upgrade() else {
                        return;
                    };
                    let Some(mut picked) = view.pick(x, y, gtk::PickFlags::DEFAULT) else {
                        return;
                    };
                    loop {
                        if let Some(path) = pane_drag.borrow().folder_for_widget(&picked) {
                            if let Some(listing) = click_model.listing()
                                && path.parent().as_ref() == Some(listing.parent())
                                && let Some(position) = listing
                                    .rows()
                                    .position(|entry| Some(entry.name()) == path.file_name())
                            {
                                let _ = input.send(AppMsg::MillerOpen(
                                    pane,
                                    column_index,
                                    position as u32,
                                ));
                            }
                            break;
                        }
                        if picked == view {
                            break;
                        }
                        let Some(parent) = picked.parent() else {
                            break;
                        };
                        picked = parent;
                    }
                });
                view.add_controller(folder_click);
                let double_click = gtk::GestureClick::new();
                double_click.set_button(1);
                double_click.set_propagation_phase(gtk::PropagationPhase::Capture);
                let input = sender.input_sender().clone();
                let weak_view = view.downgrade();
                double_click.connect_pressed(move |gesture, count, x, y| {
                    if count == 2
                        && !gesture.current_event_state().intersects(
                            gdk::ModifierType::CONTROL_MASK
                                | gdk::ModifierType::SHIFT_MASK
                                | gdk::ModifierType::ALT_MASK,
                        )
                        && let Some(view) = weak_view.upgrade()
                        && let Some((path, kind)) = context_target_at(view.upcast_ref(), x, y)
                        && !kind.is_directory()
                    {
                        let _ = input.send(AppMsg::MillerActivateFile(pane, path, kind));
                    }
                });
                view.add_controller(double_click);
                install_file_context_menu(
                    &view,
                    pane,
                    sender,
                    self.keymap.clone(),
                    Rc::clone(&self.custom_tool_store),
                    Rc::clone(&self.pane_drag),
                );
                let scroll = gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .vscrollbar_policy(gtk::PolicyType::Automatic)
                    .hexpand(true)
                    .vexpand(true)
                    .child(&view)
                    .build();
                let column_path = Rc::new(RefCell::new(Some(column.path.clone())));
                observe_scroll(
                    &scroll,
                    &scroll.vadjustment(),
                    self.pane,
                    false,
                    column_path,
                    self.restoring_scroll.clone(),
                    sender.input_sender(),
                );
                restore_scroll_after_layout(
                    &scroll,
                    column.scroll_y,
                    self.restoring_scroll.clone(),
                );
                body.append(&scroll);
                (Some(model), Some(view), Some(scroll))
            } else {
                (None, None, None)
            };
            body.append(&placeholder);
            container.append(&body);
            install_miller_resize_handle(
                &container,
                self.pane,
                &column.path,
                sender.input_sender(),
            );
            self.miller_box.append(&container);
            self.miller_columns.push(MillerColumnWidgets {
                path: column.path.clone(),
                container,
                model,
                view,
                rows,
                count,
                placeholder,
                scroll,
                loading: column.loading,
                error: column.error.clone(),
            });
        }
        for (index, (widgets, column)) in self
            .miller_columns
            .iter()
            .zip(&state.miller_columns)
            .enumerate()
        {
            widgets.container.set_width_request(column.width);
            let deepest = index + 1 == state.miller_columns.len();
            if deepest {
                widgets.container.add_css_class("miller-current");
            } else {
                widgets.container.remove_css_class("miller-current");
            }
            widgets.count.set_label(
                &column
                    .listing
                    .as_ref()
                    .map_or_else(String::new, |listing| format!("{}", listing.len())),
            );
            widgets.count.set_tooltip_text(
                column
                    .listing
                    .as_ref()
                    .map(|listing| format!("{} items", listing.len()))
                    .as_deref(),
            );
            let empty = column
                .listing
                .as_ref()
                .is_none_or(|listing| listing.is_empty());
            widgets.placeholder.set_visible(empty);
            if let Some(scroll) = &widgets.scroll {
                scroll.set_visible(!empty);
            }
            if let Some((model, listing)) = widgets.model.as_ref().zip(column.listing.clone()) {
                // GTK resets its focus tracker when rows are replaced. Remember
                // the focused entry before a refresh can bind that row to a
                // different file, including in an ancestor column.
                let focused = widgets.rows.focused_row().and_then(|row| {
                    let previous = model.listing()?;
                    let entry = previous.row(row as usize)?;
                    Some(SelectionKey::for_entry(previous.parent(), entry))
                });
                if self.rendered_scroll_restore != state.scroll_restore_epoch {
                    self.restoring_scroll.set(true);
                }
                model.set_listing(Some(Arc::clone(&listing)));
                if self.rendered_scroll_restore != state.scroll_restore_epoch
                    && let Some(scroll) = &widgets.scroll
                {
                    restore_scroll_after_layout(
                        scroll,
                        column.scroll_y,
                        self.restoring_scroll.clone(),
                    );
                }
                if tags_changed {
                    model.refresh_visible();
                }
                let mut selection = state.selection.clone();
                if !selection.is_empty() {
                    let available = listing
                        .rows()
                        .map(|entry| SelectionKey::for_entry(listing.parent(), entry))
                        .filter(|key| selection.contains(key))
                        .collect();
                    selection.retain_available(&available);
                }
                // The open branch is a navigation hint, not a GTK selection.
                // Each column's model contains only its actually selected items.
                widgets
                    .rows
                    .set_navigation_row((!deepest).then_some(column.selected_row).flatten());
                model.set_stable_selection(&selection);
                if let Some(key) = focused
                    && let Some(view) = &widgets.view
                    && let Some(row) = listing
                        .rows()
                        .position(|entry| SelectionKey::for_entry(listing.parent(), entry) == key)
                        .map(|row| row as u32)
                        .or(column.selected_row)
                    && widgets.rows.focused_row() != Some(row)
                {
                    view.scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
                    view.grab_focus();
                }
            }
        }
        let adjustment = self.miller_hadjustment.clone();
        let reveal_pending = Rc::clone(&self.miller_reveal_pending);
        glib::idle_add_local_once(move || {
            adjustment.set_value(miller_scroll_target(
                adjustment.value(),
                adjustment.lower(),
                adjustment.upper(),
                adjustment.page_size(),
                reveal_pending.replace(false),
            ));
        });
        let cursor = state
            .miller_columns
            .last()
            .map(|column| (column.path.clone(), column.selected_row));
        if self.rendered_miller_cursor != cursor {
            self.rendered_miller_cursor = cursor;
            // Refreshes, tags and async listings must not take focus from a text field.
            let file_focus = self
                .root
                .root()
                .and_downcast::<gtk::Window>()
                .and_then(|window| gtk::prelude::GtkWindowExt::focus(&window))
                .is_none_or(|focus| {
                    widget_has_ancestor_css_class(
                        &focus,
                        &["file-list", "file-grid", "column-browser"],
                    )
                });
            if active
                && file_focus
                && self.rendered_scroll_restore == state.scroll_restore_epoch
                && let Some((view, row)) = self
                    .miller_columns
                    .last()
                    .and_then(|column| column.view.as_ref())
                    .zip(
                        state
                            .miller_columns
                            .last()
                            .and_then(|column| column.selected_row),
                    )
            {
                view.scroll_to(row, gtk::ListScrollFlags::FOCUS, None);
                view.grab_focus();
            }
        }
    }

    pub(super) fn render_tabs(
        &self,
        state: &PaneState,
        archives: &archive_browser::ArchiveLocations,
        sender: &ComponentSender<AppModel>,
    ) {
        while let Some(child) = self.tab_bar.first_child() {
            self.tab_bar.remove(&child);
        }
        for (index, tab) in state.tabs.iter().enumerate() {
            let display = archives.display(&tab.path);
            let label = display
                .file_name()
                .and_then(OsStr::to_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("/");
            let pill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            pill.add_css_class("pane-tab");
            if index == state.active_tab {
                pill.add_css_class("pane-tab-active");
            }
            pill.set_hexpand(false);
            pill.set_halign(gtk::Align::Start);

            let select = gtk::Button::new();
            select.add_css_class("flat");
            select.add_css_class("tab-select");
            select.set_hexpand(true);
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 7);
            let icon = gtk::Image::from_icon_name(if archives.is_root(&tab.path) {
                "commander-archive-symbolic"
            } else {
                "commander-folder-symbolic"
            });
            icon.set_pixel_size(13);
            let tag_color = self.tag_store.borrow().get(&tab.path.to_string()).cloned();
            apply_tab_tag(&pill, &icon, tag_color.as_deref());
            let tab_label = gtk::Label::new(Some(label));
            tab_label.set_hexpand(false);
            tab_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            tab_label.set_max_width_chars(18);
            tab_label.set_single_line_mode(true);
            content.append(&icon);
            content.append(&tab_label);
            select.set_child(Some(&content));
            select.set_tooltip_text(Some(&display.to_string()));
            select.update_property(&[gtk::accessible::Property::Label(label)]);
            context_menu::install_tab_context_menu(
                &pill,
                &self.root,
                context_menu::TabFolderTarget {
                    pane: self.pane,
                    path: tab.path.clone(),
                },
                self.keymap.clone(),
                Rc::clone(&self.custom_tool_store),
                sender.input_sender().clone(),
                {
                    let pane_state = self.pane_drag.clone();
                    let location = action_policy::Location::from_archive(
                        archives
                            .contains(&tab.path)
                            .then(|| archives.read_only_reason(&tab.path).is_none()),
                    );
                    move || action_policy::Context {
                        location,
                        items: 1,
                        ..pane_state.borrow().actions
                    }
                },
            );
            let drop_destination = tab.path.clone();
            install_file_drop_target(&pill, sender, Rc::clone(&self.file_drag_ui), move |_| {
                Some(drop_destination.clone())
            });
            let input = sender.input_sender().clone();
            let pane = self.pane;
            select.connect_clicked(move |_| {
                let _ = input.send(AppMsg::SelectTab(pane, index));
            });

            let close = icon_button("commander-x-symbolic", "Close tab");
            close.add_css_class("flat");
            close.add_css_class("tab-close");
            close.set_valign(gtk::Align::Center);
            let pane = self.pane;
            connect_button(&close, sender, move || AppMsg::CloseTabAt(pane, index));

            pill.append(&select);
            pill.append(&close);
            self.tab_bar.append(&pill);
        }
    }

    pub(super) fn schedule_paint(
        &self,
        complete: bool,
        started: Instant,
        sender: &ComponentSender<AppModel>,
    ) {
        self.pending_complete.set(complete);
        if self.paint_pending.replace(true) {
            return;
        }
        tracing::debug!(
            pane = self.pane.label(),
            complete,
            mapped = self.column_view.is_mapped(),
            visible = self.column_view.is_visible(),
            "scheduled listing paint probe"
        );
        let pending = Rc::clone(&self.paint_pending);
        let pending_complete = Rc::clone(&self.pending_complete);
        let input = sender.input_sender().clone();
        let pane = self.pane;
        self.column_view.add_tick_callback(move |_, clock| {
            let handler = Rc::new(RefCell::new(None));
            let handler_for_callback = Rc::clone(&handler);
            let pending = Rc::clone(&pending);
            let pending_complete = Rc::clone(&pending_complete);
            let input = input.clone();
            let id = clock.connect_after_paint(move |clock| {
                if pending.replace(false) {
                    tracing::debug!(pane = pane.label(), "listing paint probe fired");
                    let _ = input.send(AppMsg::FramePainted {
                        pane,
                        complete: pending_complete.get(),
                        elapsed: started.elapsed(),
                    });
                }
                if let Some(id) = handler_for_callback.borrow_mut().take() {
                    clock.disconnect(id);
                }
            });
            *handler.borrow_mut() = Some(id);
            glib::ControlFlow::Break
        });
        self.column_view.queue_draw();
    }

    pub(super) fn start_scroll_benchmark(&self, sender: &ComponentSender<AppModel>) {
        tracing::debug!(pane = self.pane.label(), "starting scroll benchmark");
        self.bound_rows.set(0);
        self.max_bind_ns.set(0);
        self.max_render_ns.set(0);
        let intervals = Rc::new(RefCell::new(Vec::new()));
        let scrolling = Rc::new(Cell::new(true));
        let last_frame = Rc::new(Cell::new(0_i64));
        {
            let intervals = Rc::clone(&intervals);
            let scrolling = Rc::clone(&scrolling);
            let last_frame = Rc::clone(&last_frame);
            self.column_view.add_tick_callback(move |_, clock| {
                if !scrolling.get() {
                    return glib::ControlFlow::Break;
                }
                let frame_time = clock.frame_time();
                let previous = last_frame.replace(frame_time);
                if previous > 0 {
                    intervals
                        .borrow_mut()
                        .push((frame_time - previous) as f64 / 1_000.0);
                }
                glib::ControlFlow::Continue
            });
        }

        let step = Rc::new(Cell::new(0_u32));
        let pane = self.pane;
        let input = sender.input_sender().clone();
        let max_bind_ns = Rc::clone(&self.max_bind_ns);
        let max_render_ns = Rc::clone(&self.max_render_ns);
        let max_scroll_ns = Rc::new(Cell::new(0_u64));
        let max_scroll_ns_for_callback = Rc::clone(&max_scroll_ns);
        let bound_rows = Rc::clone(&self.bound_rows);
        let column_view = self.column_view.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let current = step.get();
            if current.is_multiple_of(30) {
                tracing::debug!(
                    pane = pane.label(),
                    step = current,
                    "scroll benchmark progress"
                );
            }
            if current >= SCROLL_BENCHMARK_STEPS {
                scrolling.set(false);
                let metrics = ScrollMetrics::from_samples(
                    &intervals.borrow(),
                    max_bind_ns.get(),
                    max_render_ns.get(),
                    max_scroll_ns.get(),
                    bound_rows.get(),
                );
                let _ = input.send(AppMsg::ScrollFinished(pane, metrics));
                return glib::ControlFlow::Break;
            }
            let callback_started = Instant::now();
            let half = SCROLL_BENCHMARK_STEPS / 2;
            let position = if current < half {
                current.saturating_mul(SCROLL_ROWS_PER_STEP)
            } else {
                (SCROLL_BENCHMARK_STEPS - current).saturating_mul(SCROLL_ROWS_PER_STEP)
            };
            column_view.scroll_to(position, None, gtk::ListScrollFlags::NONE, None);
            let elapsed = u64::try_from(callback_started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            max_scroll_ns_for_callback.set(max_scroll_ns_for_callback.get().max(elapsed));
            step.set(current + 1);
            glib::ControlFlow::Continue
        });
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScrollMetrics {
    pub(super) p95_frame_ms: f64,
    pub(super) dropped_frames: usize,
    pub(super) max_bind_us: f64,
    pub(super) max_render_us: f64,
    pub(super) max_scroll_us: f64,
    pub(super) bound_rows: u64,
}

impl ScrollMetrics {
    pub(super) fn from_samples(
        samples: &[f64],
        max_bind_ns: u64,
        max_render_ns: u64,
        max_scroll_ns: u64,
        bound_rows: u64,
    ) -> Self {
        let mut ordered = samples.to_vec();
        ordered.sort_by(f64::total_cmp);
        let p95_index = ordered.len().saturating_sub(1) * 95 / 100;
        let p95_frame_ms = ordered.get(p95_index).copied().unwrap_or_default();
        let dropped_frames = ordered.iter().filter(|&&value| value > 25.0).count();
        Self {
            p95_frame_ms,
            dropped_frames,
            max_bind_us: max_bind_ns as f64 / 1_000.0,
            max_render_us: max_render_ns as f64 / 1_000.0,
            max_scroll_us: max_scroll_ns as f64 / 1_000.0,
            bound_rows,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ColumnKind {
    Name,
    Size,
    Modified,
}

pub(super) const TAG_CLASSES: &[&str] = &[
    "tag-red",
    "tag-orange",
    "tag-yellow",
    "tag-green",
    "tag-blue",
    "tag-purple",
];

pub(super) fn apply_tab_tag(pill: &gtk::Box, icon: &gtk::Image, color: Option<&str>) {
    let Some(color) = color else {
        return;
    };
    let icon_class = format!("tag-{color}");
    if !TAG_CLASSES.contains(&icon_class.as_str()) {
        return;
    }

    pill.add_css_class("pane-tab-tagged");
    pill.add_css_class(&format!("pane-tab-tag-{color}"));
    icon.add_css_class(&icon_class);
}

pub(super) fn tag_indicator() -> gtk::Label {
    let indicator = gtk::Label::new(Some("●"));
    indicator.add_css_class("tag-indicator");
    indicator.set_visible(false);
    indicator
}

pub(super) fn apply_tag_indicator(indicator: &gtk::Label, color: Option<&String>) {
    for class in TAG_CLASSES {
        indicator.remove_css_class(class);
    }
    let Some(color) = color else {
        indicator.set_visible(false);
        return;
    };
    let class = format!("tag-{color}");
    if TAG_CLASSES.contains(&class.as_str()) {
        indicator.add_css_class(&class);
        indicator.set_visible(true);
    } else {
        indicator.set_visible(false);
    }
}

pub(super) fn set_context_target_metadata(
    widget: &impl IsA<gtk::Widget>,
    path: &VPath,
    kind: EntryKind,
) {
    let widget = widget.as_ref();
    widget.set_tooltip_text(path.as_path().to_str());
    let name = path
        .file_name()
        .map_or_else(|| path.to_string(), display_name);
    let accessible_label = format!(
        "{name}, {}",
        if kind == EntryKind::Directory {
            "folder"
        } else {
            "file"
        }
    );
    widget.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    widget.remove_css_class("context-directory-target");
    widget.remove_css_class("context-file-target");
    for (special_kind, class) in CONTEXT_SPECIAL_KINDS {
        if kind == *special_kind {
            widget.add_css_class(class);
        } else {
            widget.remove_css_class(class);
        }
    }
    widget.add_css_class(if kind == EntryKind::Directory {
        "context-directory-target"
    } else {
        "context-file-target"
    });
}

pub(super) fn build_grid_factory(
    pane: PaneId,
    generation: Rc<Cell<u64>>,
    thumbnail_ui: Rc<RefCell<ThumbnailUiState>>,
    pane_drag: Rc<RefCell<PaneDragState>>,
    file_drag_ui: Rc<FileDragUiState>,
    sender: &ComponentSender<AppModel>,
    tag_store: Rc<RefCell<BTreeMap<String, String>>>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let setup_pane_drag = Rc::clone(&pane_drag);
    let setup_file_drag_ui = Rc::clone(&file_drag_ui);
    let setup_sender = sender.clone();
    factory.connect_setup(move |_, item| {
        let item = item
            .downcast_ref::<gtk::ListItem>()
            .expect("factory item must be a ListItem");
        let card = gtk::Box::new(gtk::Orientation::Vertical, 9);
        card.add_css_class("file-card");
        card.set_halign(gtk::Align::Center);
        let preview = gtk::Stack::new();
        preview.add_css_class("file-card-icon");
        preview.set_size_request(96, 72);
        preview.set_overflow(gtk::Overflow::Hidden);
        let icon = gtk::Image::new();
        icon.add_css_class("file-card-generic-icon");
        icon.set_pixel_size(48);
        let picture = gtk::Picture::new();
        picture.add_css_class("file-card-thumbnail");
        picture.set_size_request(96, 72);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Cover);
        preview.add_named(&icon, Some("icon"));
        preview.add_named(&picture, Some("thumbnail"));
        preview.set_visible_child_name("icon");
        let label = gtk::Label::new(None);
        label.add_css_class("file-card-name");
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(18);
        label.set_lines(1);
        label.set_justify(gtk::Justification::Center);
        label.set_wrap(false);
        let tag = tag_indicator();
        card.append(&preview);
        card.append(&label);
        card.append(&tag);
        install_file_drag_source(
            &card,
            Rc::clone(&setup_pane_drag),
            Rc::clone(&setup_file_drag_ui),
        );
        let drop_pane_drag = Rc::clone(&setup_pane_drag);
        install_file_drop_target(
            &card,
            &setup_sender,
            Rc::clone(&setup_file_drag_ui),
            move |widget| drop_pane_drag.borrow().folder_for_widget(widget),
        );
        item.set_child(Some(&card));
    });
    let input = sender.input_sender().clone();
    let bind_thumbnail_ui = Rc::clone(&thumbnail_ui);
    let bind_pane_drag = Rc::clone(&pane_drag);
    factory.connect_bind(move |_, item| {
        let item = item
            .downcast_ref::<gtk::ListItem>()
            .expect("factory item must be a ListItem");
        let Some(card) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(preview) = card.first_child().and_downcast::<gtk::Stack>() else {
            return;
        };
        let Some(icon) = preview.child_by_name("icon").and_downcast::<gtk::Image>() else {
            return;
        };
        let Some(picture) = preview
            .child_by_name("thumbnail")
            .and_downcast::<gtk::Picture>()
        else {
            return;
        };
        let Some(label) = preview.next_sibling().and_downcast::<gtk::Label>() else {
            return;
        };
        let Some(tag) = card.last_child().and_downcast::<gtk::Label>() else {
            return;
        };
        let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let row = boxed.borrow::<RowReference>();
        let Some(entry) = row.listing.entry(row.source_index) else {
            return;
        };
        crate::icons::set_file_icon(&icon, entry.kind(), entry.name());
        label.set_label(&display_name(entry.name()));
        let path = row.listing.parent().join_name(entry.name());
        set_context_target_metadata(&card, &path, entry.kind());
        bind_pane_drag
            .borrow_mut()
            .bind(&card, path.clone(), entry.kind());
        apply_tag_indicator(&tag, tag_store.borrow().get(&path.to_string()));
        let fingerprint = row
            .listing
            .metadata(row.source_index)
            .map(ThumbnailFingerprint::from_metadata);
        if entry.kind() == EntryKind::File
            && supports_thumbnail(&path)
            && bind_thumbnail_ui.borrow_mut().bind(
                pane,
                generation.get(),
                &path,
                fingerprint,
                &preview,
                &picture,
            )
        {
            let _ = input.send(AppMsg::RequestThumbnail {
                pane,
                generation: generation.get(),
                path,
            });
        } else if entry.kind() != EntryKind::File || !supports_thumbnail(&path) {
            bind_thumbnail_ui.borrow_mut().unbind(&preview, &picture);
        }
    });
    factory.connect_unbind(move |_, item| {
        let Some(card) = item
            .downcast_ref::<gtk::ListItem>()
            .and_then(gtk::ListItem::child)
            .and_downcast::<gtk::Box>()
        else {
            return;
        };
        let Some(preview) = card.first_child().and_downcast::<gtk::Stack>() else {
            return;
        };
        pane_drag.borrow_mut().unbind(&card);
        let Some(picture) = preview
            .child_by_name("thumbnail")
            .and_downcast::<gtk::Picture>()
        else {
            return;
        };
        thumbnail_ui.borrow_mut().unbind(&preview, &picture);
    });
    factory
}

pub(super) fn build_column_browser_factory(
    rows: Rc<MillerRows>,
    tag_store: Rc<RefCell<BTreeMap<String, String>>>,
    pane_drag: Rc<RefCell<PaneDragState>>,
    file_drag_ui: Rc<FileDragUiState>,
    sender: &ComponentSender<AppModel>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let setup_rows = Rc::clone(&rows);
    let setup_pane_drag = Rc::clone(&pane_drag);
    let setup_file_drag_ui = Rc::clone(&file_drag_ui);
    let setup_sender = sender.clone();
    factory.connect_setup(move |_, item| {
        let item = item
            .downcast_ref::<gtk::ListItem>()
            .expect("factory item must be a ListItem");
        let mut items = setup_rows.items.borrow_mut();
        items.retain(|weak| weak.upgrade().is_some());
        items.push(item.downgrade());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("column-browser-row");
        let icon = gtk::Image::new();
        icon.set_pixel_size(17);
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_width_chars(1);
        label.set_max_width_chars(1);
        let arrow = gtk::Image::from_icon_name("commander-chevron-right-symbolic");
        arrow.add_css_class("column-browser-chevron");
        arrow.set_pixel_size(12);
        row.append(&icon);
        row.append(&tag_indicator());
        row.append(&label);
        row.append(&arrow);
        install_file_drag_source(
            &row,
            Rc::clone(&setup_pane_drag),
            Rc::clone(&setup_file_drag_ui),
        );
        let drop_pane_drag = Rc::clone(&setup_pane_drag);
        install_file_drop_target(
            &row,
            &setup_sender,
            Rc::clone(&setup_file_drag_ui),
            move |widget| drop_pane_drag.borrow().folder_for_widget(widget),
        );
        item.set_child(Some(&row));
    });
    let bind_pane_drag = Rc::clone(&pane_drag);
    factory.connect_bind(move |_, item| {
        let item = item
            .downcast_ref::<gtk::ListItem>()
            .expect("factory item must be a ListItem");
        rows.style_item(item);
        let Some(container) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(icon) = container.first_child().and_downcast::<gtk::Image>() else {
            return;
        };
        let Some(tag) = icon.next_sibling().and_downcast::<gtk::Label>() else {
            return;
        };
        let Some(label) = tag.next_sibling().and_downcast::<gtk::Label>() else {
            return;
        };
        let Some(arrow) = container.last_child().and_downcast::<gtk::Image>() else {
            return;
        };
        let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let row = boxed.borrow::<RowReference>();
        let Some(entry) = row.listing.entry(row.source_index) else {
            return;
        };
        crate::icons::set_file_icon(&icon, entry.kind(), entry.name());
        label.set_label(&display_name(entry.name()));
        container.set_tooltip_text(Some(&display_name(entry.name())));
        arrow.set_visible(entry.kind().is_directory());
        let path = row.listing.parent().join_name(entry.name());
        set_context_target_metadata(&container, &path, entry.kind());
        bind_pane_drag
            .borrow_mut()
            .bind(&container, path.clone(), entry.kind());
        apply_tag_indicator(&tag, tag_store.borrow().get(&path.to_string()));
    });
    factory.connect_unbind(move |_, item| {
        let Some(container) = item
            .downcast_ref::<gtk::ListItem>()
            .and_then(gtk::ListItem::child)
            .and_downcast::<gtk::Box>()
        else {
            return;
        };
        pane_drag.borrow_mut().unbind(&container);
    });
    factory
}

impl ColumnKind {
    pub(super) const fn title(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Modified => "Modified",
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_column(
    view: &gtk::ColumnView,
    pane: PaneId,
    kind: ColumnKind,
    sender: &ComponentSender<AppModel>,
    bound_rows: Rc<Cell<u64>>,
    max_bind_ns: Rc<Cell<u64>>,
    metadata_request_pending: Rc<Cell<bool>>,
    tag_store: Rc<RefCell<BTreeMap<String, String>>>,
    pane_drag: Rc<RefCell<PaneDragState>>,
    file_drag_ui: Rc<FileDragUiState>,
) {
    let factory = gtk::SignalListItemFactory::new();
    let setup_pane_drag = Rc::clone(&pane_drag);
    let setup_file_drag_ui = Rc::clone(&file_drag_ui);
    let setup_sender = sender.clone();
    factory.connect_setup(move |_, item| {
        let item = item
            .downcast_ref::<gtk::ListItem>()
            .expect("factory item must be a ListItem");
        let label = gtk::Label::new(None);
        label.set_xalign(if matches!(kind, ColumnKind::Size) {
            1.0
        } else {
            0.0
        });
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        if matches!(kind, ColumnKind::Name) {
            label.set_hexpand(true);
            label.set_width_chars(1);
            label.set_max_width_chars(1);
            let cell = gtk::Box::new(gtk::Orientation::Horizontal, 7);
            cell.add_css_class("file-name-cell");
            cell.set_margin_start(8);
            cell.set_margin_end(6);
            cell.set_margin_top(3);
            cell.set_margin_bottom(3);
            let icon = gtk::Image::new();
            icon.set_pixel_size(16);
            cell.append(&icon);
            cell.append(&tag_indicator());
            cell.append(&label);
            install_file_drag_source(
                &cell,
                Rc::clone(&setup_pane_drag),
                Rc::clone(&setup_file_drag_ui),
            );
            let drop_pane_drag = Rc::clone(&setup_pane_drag);
            install_file_drop_target(
                &cell,
                &setup_sender,
                Rc::clone(&setup_file_drag_ui),
                move |widget| drop_pane_drag.borrow().folder_for_widget(widget),
            );
            item.set_child(Some(&cell));
        } else {
            label.set_margin_start(6);
            label.set_margin_end(8);
            label.set_margin_top(3);
            label.set_margin_bottom(3);
            item.set_child(Some(&label));
        }
    });
    let input = sender.input_sender().clone();
    let bind_pane_drag = Rc::clone(&pane_drag);
    factory.connect_bind(move |_, item| {
        let started = Instant::now();
        let item = item
            .downcast_ref::<gtk::ListItem>()
            .expect("factory item must be a ListItem");
        let child = item.child();
        let label = if matches!(kind, ColumnKind::Name) {
            child
                .as_ref()
                .and_then(|child| child.downcast_ref::<gtk::Box>())
                .and_then(|cell| cell.last_child())
                .and_downcast::<gtk::Label>()
        } else {
            child.and_downcast::<gtk::Label>()
        };
        let Some(label) = label else { return };
        let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            label.set_label("");
            return;
        };
        let row = boxed.borrow::<RowReference>();
        let Some(entry) = row.listing.entry(row.source_index) else {
            label.set_label("");
            return;
        };
        let metadata = row.listing.metadata(row.source_index);
        let path = row.listing.parent().join_name(entry.name());
        if let Some(child) = item.child() {
            set_context_target_metadata(&child, &path, entry.kind());
            if matches!(kind, ColumnKind::Name) {
                bind_pane_drag
                    .borrow_mut()
                    .bind(&child, path.clone(), entry.kind());
            }
        }
        if matches!(kind, ColumnKind::Name)
            && let Some(cell) = item.child().and_downcast::<gtk::Box>()
            && let Some(icon) = cell.first_child().and_downcast::<gtk::Image>()
        {
            crate::icons::set_file_icon(&icon, entry.kind(), entry.name());
            if let Some(tag) = icon.next_sibling().and_downcast::<gtk::Label>() {
                apply_tag_indicator(&tag, tag_store.borrow().get(&path.to_string()));
            }
        }
        let text = match kind {
            ColumnKind::Name => display_name(entry.name()),
            ColumnKind::Size => metadata.map_or_else(
                || "—".to_owned(),
                |metadata| format_size(metadata.size, metadata.kind),
            ),
            ColumnKind::Modified => metadata
                .and_then(|metadata| metadata.modified)
                .map_or_else(|| "—".to_owned(), |value| format_timestamp(value.seconds)),
        };
        label.set_label(&text);
        if matches!(kind, ColumnKind::Name)
            && metadata.is_none()
            && !metadata_request_pending.replace(true)
        {
            let _ = input.send(AppMsg::MetadataVisible(pane, row.row_index));
        }
        bound_rows.set(bound_rows.get().saturating_add(1));
        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        max_bind_ns.set(max_bind_ns.get().max(elapsed));
    });
    factory.connect_unbind(move |_, item| {
        if !matches!(kind, ColumnKind::Name) {
            return;
        }
        if let Some(child) = item
            .downcast_ref::<gtk::ListItem>()
            .and_then(gtk::ListItem::child)
        {
            pane_drag.borrow_mut().unbind(&child);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(kind.title()), Some(factory));
    column.set_resizable(true);
    if matches!(kind, ColumnKind::Name) {
        column.set_fixed_width(80);
        column.set_expand(true);
    } else {
        column.set_fixed_width(match kind {
            ColumnKind::Size => 82,
            ColumnKind::Modified => 128,
            ColumnKind::Name => -1,
        });
    }
    view.append_column(&column);
}

fn observe_scroll(
    scroller: &gtk::ScrolledWindow,
    adjustment: &gtk::Adjustment,
    pane: PaneId,
    x: bool,
    path: Rc<RefCell<Option<VPath>>>,
    restoring: Rc<Cell<bool>>,
    input: &relm4::Sender<AppMsg>,
) {
    let pending = Rc::new(RefCell::new(None::<glib::SourceId>));
    let scroller = scroller.downgrade();
    let input = input.clone();
    adjustment.connect_value_changed(move |adjustment| {
        if restoring.get() || !scroller.upgrade().is_some_and(|widget| widget.is_mapped()) {
            return;
        }
        let Some(path) = path.borrow().clone() else {
            return;
        };
        if let Some(previous) = pending.borrow_mut().take() {
            previous.remove();
        }
        let value = adjustment.value().max(0.0) as u32;
        let input = input.clone();
        let pending_next = pending.clone();
        *pending.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_millis(150),
            move || {
                pending_next.borrow_mut().take();
                let _ = input.send(AppMsg::NavigationScroll {
                    pane,
                    path,
                    x,
                    value,
                });
            },
        ));
    });
}

fn restore_scroll_after_layout(
    scroller: &gtk::ScrolledWindow,
    value: u32,
    restoring: Rc<Cell<bool>>,
) {
    let scroller = scroller.downgrade();
    restoring.set(true);
    glib::timeout_add_local_once(Duration::from_millis(40), move || {
        if let Some(scroller) = scroller.upgrade() {
            let adjustment = scroller.vadjustment();
            adjustment.set_value(
                (value as f64).min((adjustment.upper() - adjustment.page_size()).max(0.0)),
            );
        }
        restoring.set(false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_drop_uses_file_manager_copy_move_conventions() {
        let offered = gdk::DragAction::COPY | gdk::DragAction::MOVE;
        assert_eq!(
            choose_file_drop_action(offered, gdk::ModifierType::empty(), true),
            gdk::DragAction::MOVE
        );
        assert_eq!(
            choose_file_drop_action(offered, gdk::ModifierType::empty(), false),
            gdk::DragAction::COPY
        );
        assert_eq!(
            choose_file_drop_action(offered, gdk::ModifierType::CONTROL_MASK, true),
            gdk::DragAction::COPY
        );
        assert_eq!(
            choose_file_drop_action(offered, gdk::ModifierType::SHIFT_MASK, false),
            gdk::DragAction::MOVE
        );
        assert_eq!(
            choose_file_drop_action(gdk::DragAction::COPY, gdk::ModifierType::SHIFT_MASK, true,),
            gdk::DragAction::COPY
        );
    }
}

#[cfg(test)]
mod archive_drop_tests {
    use super::*;

    #[test]
    fn archive_drops_advertise_copy_and_reject_read_only_or_explicit_moves() {
        let actions = gdk::DragAction::COPY | gdk::DragAction::MOVE;
        assert_eq!(
            archive_file_drop_action(actions, gdk::ModifierType::empty(), true, Some(true)),
            gdk::DragAction::COPY
        );
        assert_eq!(
            archive_file_drop_action(actions, gdk::ModifierType::SHIFT_MASK, true, Some(true)),
            gdk::DragAction::empty()
        );
        assert_eq!(
            archive_file_drop_action(actions, gdk::ModifierType::empty(), true, Some(false)),
            gdk::DragAction::empty()
        );
        assert_eq!(
            archive_file_drop_action(actions, gdk::ModifierType::empty(), true, None),
            gdk::DragAction::MOVE
        );
    }
}
