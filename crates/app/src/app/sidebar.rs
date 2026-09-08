//! The collapsible sidebar: favorites, workspaces, devices, remotes, and recent paths.

use super::*;

mod collapse;
pub(super) mod menus;
pub(super) use collapse::{CollapsibleGroup, favorite_group_key};

/// The sidebar's widgets, plus the fixed rows that can light up as the current location.
pub(super) struct SidebarWidgets {
    pub(super) revealer: gtk::Revealer,
    pub(super) focus_target: gtk::Button,
    pub(super) groups: Vec<CollapsibleGroup>,
    pub(super) bookmarks: gtk::Box,
    pub(super) recent: gtk::Box,
    pub(super) workspaces: gtk::Box,
    pub(super) remotes: gtk::Box,
    pub(super) devices: devices::DeviceSidebar,
    pub(super) places: Vec<(VPath, gtk::Button)>,
}

pub(super) fn build_sidebar(
    window: &adw::ApplicationWindow,
    sender: &ComponentSender<AppModel>,
    file_drag_ui: Rc<FileDragUiState>,
    collapsed: &BTreeSet<String>,
) -> SidebarWidgets {
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
    revealer.set_hexpand(false);
    revealer.set_vexpand(true);
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 4);
    sidebar.add_css_class("sidebar-surface");
    sidebar.set_hexpand(false);
    sidebar.set_width_request(SIDEBAR_WIDTH);

    let window_region = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    window_region.add_css_class("sidebar-window-region");
    window_region.append(&window_controls(window));
    let window_handle = gtk::WindowHandle::new();
    window_handle.set_child(Some(&window_region));
    sidebar.append(&window_handle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body.add_css_class("sidebar-scroll-content");

    let mut places = Vec::new();
    let mut groups = Vec::new();
    let place_group = sidebar_section("places", "Places", None, collapsed, sender);
    place_group.append_to(&body);
    let home = sidebar_button("Home", "commander-house-symbolic");
    let home_destination = home_path();
    home.set_tooltip_text(Some(&home_destination.to_string()));
    menus::location(
        &home,
        "Home",
        &home_destination,
        true,
        sender.input_sender(),
    );
    places.push((home_destination.clone(), home.clone()));
    let mut place_paths = BTreeSet::from([home_destination.clone()]);
    {
        let input = sender.input_sender().clone();
        let destination = home_destination.clone();
        home.connect_clicked(move |_| {
            let _ = input.send(AppMsg::NavigateActive(destination.clone()));
        });
    }
    install_file_drop_target(&home, sender, Rc::clone(&file_drag_ui), move |_| {
        Some(home_destination.clone())
    });
    place_group.content.append(&home);

    for (label, icon, directory) in [
        (
            "Desktop",
            "commander-monitor-symbolic",
            glib::UserDirectory::Desktop,
        ),
        (
            "Documents",
            "commander-files-symbolic",
            glib::UserDirectory::Documents,
        ),
        (
            "Downloads",
            "commander-download-symbolic",
            glib::UserDirectory::Downloads,
        ),
        (
            "Music",
            "commander-music-2-symbolic",
            glib::UserDirectory::Music,
        ),
        (
            "Pictures",
            "commander-images-symbolic",
            glib::UserDirectory::Pictures,
        ),
        (
            "Videos",
            "commander-film-symbolic",
            glib::UserDirectory::Videos,
        ),
    ] {
        let Some(path) = glib::user_special_dir(directory) else {
            continue;
        };
        let destination = VPath::from(path);
        if !destination.as_path().is_dir() || !place_paths.insert(destination.clone()) {
            continue;
        }
        let button = sidebar_button(label, icon);
        button.set_tooltip_text(Some(&destination.to_string()));
        menus::location(&button, label, &destination, true, sender.input_sender());
        places.push((destination.clone(), button.clone()));
        {
            let input = sender.input_sender().clone();
            let destination = destination.clone();
            button.connect_clicked(move |_| {
                let _ = input.send(AppMsg::NavigateActive(destination.clone()));
            });
        }
        install_file_drop_target(&button, sender, Rc::clone(&file_drag_ui), move |_| {
            Some(destination.clone())
        });
        place_group.content.append(&button);
    }

    let focus_target = place_group.toggle.clone().upcast();
    groups.push(place_group);
    let favorite_group = sidebar_section(
        "favorites",
        "Favorites",
        Some((
            "commander-plus-symbolic",
            "Add the current location to favorites",
            CommandId::Bookmark,
        )),
        collapsed,
        sender,
    );
    let bookmarks = favorite_group.content.clone();
    let favorite_drop = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
    {
        let input = sender.input_sender().clone();
        favorite_drop.connect_drop(move |_, value, _, _| {
            let Ok(value) = value.get::<String>() else {
                return false;
            };
            let Some(index) = value
                .strip_prefix("favorite-index:")
                .and_then(|index| index.parse::<usize>().ok())
            else {
                return false;
            };
            let _ = input.send(AppMsg::MoveBookmarkToEnd(index));
            true
        });
    }
    bookmarks.add_controller(favorite_drop);
    favorite_group.append_to(&body);
    groups.push(favorite_group);

    // Permanent locations rank above history: drives and servers, then Recent and Workspaces.
    let devices = devices::DeviceSidebar::new(sender);
    let device_group = sidebar_section("devices", "Devices", None, collapsed, sender);
    device_group.content.append(&devices.devices);
    device_group.append_to(&body);
    groups.push(device_group);

    let remote_group = sidebar_section(
        "remotes",
        "Remote Storage",
        Some((
            "commander-plus-symbolic",
            "Connect to a server",
            CommandId::ConnectRemote,
        )),
        collapsed,
        sender,
    );
    let remotes = gtk::Box::new(gtk::Orientation::Vertical, 2);
    remote_group.content.append(&remotes);
    remote_group.content.append(&devices.mounts);
    remote_group.append_to(&body);
    groups.push(remote_group);

    let workspace_group = sidebar_section(
        "workspaces",
        "Workspaces",
        Some((
            "commander-plus-symbolic",
            "Save the current setup as a workspace",
            CommandId::SaveWorkspace,
        )),
        collapsed,
        sender,
    );
    let workspaces = workspace_group.content.clone();
    workspace_group.append_to(&body);
    groups.push(workspace_group);

    let recent_group = sidebar_section("recent", "Recent", None, collapsed, sender);
    let recent = recent_group.content.clone();
    recent_group.append_to(&body);
    groups.push(recent_group);

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_width(false)
        .vexpand(true)
        .child(&body)
        .build();
    scrolled.add_css_class("sidebar-scroller");
    sidebar.append(&scrolled);

    // Trash is pinned outside the scroll area: it is a frequent target and should never
    // need scrolling to reach.
    let pinned = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pinned.add_css_class("sidebar-pinned");
    let trash_destination = local_trash_path();
    let trash = sidebar_button("Trash", "commander-trash-symbolic");
    trash.set_tooltip_text(Some("Open the local trash contents"));
    menus::location(
        &trash,
        "Trash",
        &trash_destination,
        false,
        sender.input_sender(),
    );
    places.push((trash_destination.clone(), trash.clone()));
    {
        let input = sender.input_sender().clone();
        trash.connect_clicked(move |_| {
            let _ = input.send(AppMsg::NavigateActive(local_trash_path()));
        });
    }
    pinned.append(&trash);
    sidebar.append(&pinned);

    revealer.set_child(Some(&sidebar));
    SidebarWidgets {
        revealer,
        focus_target,
        groups,
        bookmarks,
        recent,
        workspaces,
        remotes,
        devices,
        places,
    }
}

/// A section heading with an optional always-visible action on its trailing edge.
pub(super) fn sidebar_section(
    key: &str,
    label: &str,
    action: Option<(&str, &str, CommandId)>,
    collapsed: &BTreeSet<String>,
    sender: &ComponentSender<AppModel>,
) -> CollapsibleGroup {
    let group = CollapsibleGroup::new(key, label, !collapsed.contains(key), sender.input_sender());
    if let Some((icon, tooltip, command)) = action {
        let button = icon_button(icon, tooltip);
        button.add_css_class("flat");
        button.add_css_class("sidebar-heading-action");
        let toggle = group.toggle.clone();
        let input = sender.input_sender().clone();
        button.connect_clicked(move |_| {
            toggle.set_active(true);
            let _ = input.send(AppMsg::ExecuteCommand(command));
        });
        group.heading.append(&button);
    }
    group
}

pub(super) fn sidebar_button(label: &str, icon: &str) -> gtk::Button {
    let accessible_label = label.to_owned();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(16);
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(SIDEBAR_LABEL_WIDTH_CHARS);
    label.set_single_line_mode(true);
    content.append(&image);
    content.append(&label);
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("sidebar-row");
    button.set_halign(gtk::Align::Fill);
    button.set_child(Some(&content));
    button.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    button
}

/// The close, minimize, and maximize controls. The sidebar hosts one set; the
/// top bar shows another whenever the sidebar is hidden, so the window can
/// always be managed from the same corner.
pub(super) fn window_controls(window: &adw::ApplicationWindow) -> gtk::Box {
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    controls.add_css_class("window-controls");
    controls.set_valign(gtk::Align::Center);
    let close = chrome::close_button();
    controls.append(&close);
    let minimize = gtk::Button::from_icon_name("commander-minus-symbolic");
    let maximize = gtk::Button::from_icon_name("commander-square-symbolic");
    for (button, class, tooltip) in [
        (&minimize, "window-minimize", "Minimize"),
        (&maximize, "window-maximize", "Maximize"),
    ] {
        button.add_css_class("window-action");
        button.add_css_class(class);
        button.set_tooltip_text(Some(tooltip));
        button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
        button.set_size_request(24, 24);
        button.set_valign(gtk::Align::Center);
        controls.append(button);
    }
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
    }
    {
        let window = window.clone();
        minimize.connect_clicked(move |_| window.minimize());
    }
    {
        let window = window.clone();
        maximize.connect_clicked(move |_| {
            if window.is_maximized() {
                window.unmaximize();
            } else {
                window.maximize();
            }
        });
    }
    controls
}
