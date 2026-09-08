//! The collapsible sidebar: favorites, workspaces, devices, remotes, and recent paths.

use super::*;

/// The sidebar's widgets, plus the fixed rows that can light up as the current location.
pub(super) struct SidebarWidgets {
    pub(super) revealer: gtk::Revealer,
    pub(super) focus_target: gtk::Button,
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
    body.append(&sidebar_heading("Places"));
    let home = sidebar_button("Home", "commander-house-symbolic");
    let home_destination = home_path();
    home.set_tooltip_text(Some(&home_destination.to_string()));
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
    body.append(&home);

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
        body.append(&button);
    }

    body.append(&sidebar_section(
        "Favorites",
        Some((
            "commander-plus-symbolic",
            "Add the current location to favorites",
            CommandId::Bookmark,
        )),
        sender,
    ));
    let bookmarks = gtk::Box::new(gtk::Orientation::Vertical, 2);
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
    body.append(&bookmarks);

    // Permanent locations rank above history: drives and servers, then Recent and Workspaces.
    let devices = devices::DeviceSidebar::new(sender);
    body.append(&devices.devices);

    body.append(&sidebar_section(
        "Remote Storage",
        Some((
            "commander-plus-symbolic",
            "Connect to a server",
            CommandId::ConnectRemote,
        )),
        sender,
    ));
    let remotes = gtk::Box::new(gtk::Orientation::Vertical, 2);
    body.append(&remotes);
    body.append(&devices.mounts);

    body.append(&sidebar_section(
        "Workspaces",
        Some((
            "commander-plus-symbolic",
            "Save the current setup as a workspace",
            CommandId::SaveWorkspace,
        )),
        sender,
    ));
    let workspaces = gtk::Box::new(gtk::Orientation::Vertical, 2);
    body.append(&workspaces);

    body.append(&sidebar_heading("Recent"));
    let recent = gtk::Box::new(gtk::Orientation::Vertical, 2);
    body.append(&recent);

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
        focus_target: home,
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
    label: &str,
    action: Option<(&str, &str, CommandId)>,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("sidebar-heading-row");
    let heading = sidebar_heading(label);
    heading.set_hexpand(true);
    row.append(&heading);
    if let Some((icon, tooltip, command)) = action {
        let button = icon_button(icon, tooltip);
        button.add_css_class("flat");
        button.add_css_class("sidebar-heading-action");
        connect_button(&button, sender, move || AppMsg::ExecuteCommand(command));
        row.append(&button);
    }
    row
}

pub(super) fn sidebar_heading(label: &str) -> gtk::Label {
    let heading = gtk::Label::new(Some(label));
    heading.add_css_class("sidebar-heading");
    heading.set_xalign(0.0);
    heading
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
    let close = gtk::Button::from_icon_name("commander-x-symbolic");
    let minimize = gtk::Button::from_icon_name("commander-minus-symbolic");
    let maximize = gtk::Button::from_icon_name("commander-square-symbolic");
    for (button, class, tooltip) in [
        (&close, "window-close", "Close"),
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
