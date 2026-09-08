use super::*;
use relm4::{Component, ComponentController};

#[test]
fn removal_checks_sources_destinations_and_path_boundaries() {
    let roots = vec![
        VPath::from(std::path::Path::new("/media/usb")),
        VPath::from(std::path::Path::new("/media/second-partition")),
    ];
    let path = |value| VPath::from(std::path::Path::new(value));
    for (source, destination, expected) in [
        ("/media/usb/folder/file", "/home/user", true),
        ("/home/user/file", "/media/usb", true),
        ("/media/second-partition/file", "/home/user", true),
        ("/media/usb-backup/file", "/home/user", false),
    ] {
        let retry = OperationRetry::Copy {
            pane: PaneId::Left,
            sources: vec![path(source)],
            destination: path(destination),
        };
        assert_eq!(operation_uses_roots(&retry, &roots), expected);
    }
    assert!(operation_uses_roots(
        &OperationRetry::Trash {
            pane: PaneId::Left,
            sources: vec![path("/media/usb/file")],
        },
        &roots
    ));
}

#[test]
fn busy_canceled_and_failed_removals_have_distinct_feedback() {
    let busy = glib::Error::new(gio::IOErrorEnum::Busy, "Device busy");
    let canceled = glib::Error::new(gio::IOErrorEnum::Cancelled, "Cancelled");
    let denied = glib::Error::new(gio::IOErrorEnum::PermissionDenied, "Permission denied");
    assert!(removal_error("USB", &busy, false).contains("Close files and terminals"));
    assert!(removal_error("USB", &canceled, true).contains("is busy"));
    assert!(removal_error("USB", &canceled, false).contains("was canceled"));
    assert!(removal_error("USB", &denied, false).contains("Permission denied"));
}

struct FakeDevices {
    entries: Rc<RefCell<Vec<DeviceEntry>>>,
    roots: Vec<VPath>,
    failure: RefCell<Option<String>>,
    calls: Cell<usize>,
}

impl DeviceBackend for FakeDevices {
    fn entries(&self) -> Vec<DeviceEntry> {
        self.entries.borrow().clone()
    }

    fn prepare(&self, key: &str, remove: bool) -> Result<PreparedAction, String> {
        let entry = self
            .entries
            .borrow()
            .iter()
            .find(|entry| entry.key == key)
            .cloned()
            .ok_or("This device is no longer connected.")?;
        if remove && entry.removal.is_none() {
            return Err("Removal is unavailable.".to_owned());
        }
        self.calls.set(self.calls.get() + 1);
        let failure = self.failure.borrow_mut().take();
        let entries = self.entries.clone();
        let roots = self.roots.clone();
        Ok(PreparedAction {
            roots: roots.clone(),
            progress: format!("Ejecting {}…", entry.name),
            future: Box::pin(async move {
                glib::timeout_future(Duration::from_millis(100)).await;
                if let Some(error) = failure {
                    return Err(error);
                }
                if remove {
                    entries
                        .borrow_mut()
                        .retain(|candidate| candidate.key != entry.key);
                    Ok(DeviceOutcome::Removed {
                        roots,
                        message: "USB Drive ejected. You can disconnect it.".to_owned(),
                    })
                } else {
                    let root = entry.root.unwrap_or_else(|| roots[0].clone());
                    entries
                        .borrow_mut()
                        .iter_mut()
                        .find(|candidate| candidate.key == entry.key)
                        .unwrap()
                        .root = Some(root.clone());
                    Ok(DeviceOutcome::Open(root))
                }
            }),
        })
    }
}

#[test]
#[ignore = "requires an isolated GTK session; run alone with --ignored --test-threads=1"]
fn gtk_device_removal_updates_sidebar_and_handles_busy_drives() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let usb = fixture.path().join("usb");
    let second = fixture.path().join("second-partition");
    std::fs::create_dir_all(usb.join("projects")).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let roots = vec![VPath::from(usb.as_path()), VPath::from(second.as_path())];
    let fake = Rc::new(FakeDevices {
        entries: Rc::new(RefCell::new(vec![
            DeviceEntry {
                key: "usb".to_owned(),
                name: "USB Drive".to_owned(),
                root: Some(roots[0].clone()),
                remote: false,
                removal: Some(Removal::Stop),
                removal_name: "USB Drive (all volumes)".to_owned(),
            },
            DeviceEntry {
                key: "system".to_owned(),
                name: "System Disk".to_owned(),
                root: Some(home_path()),
                remote: false,
                removal: None,
                removal_name: "System Disk".to_owned(),
            },
        ])),
        roots: roots.clone(),
        failure: RefCell::new(None),
        calls: Cell::new(0),
    });
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(roots[0].clone()),
                right: Some(roots[1].clone()),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                sidebar_visible: true,
                preview_visible: false,
                ..SessionState::default()
            }),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    app.state().get_mut().model.devices.backend = fake.clone();
    app.emit(AppMsg::DevicesChanged);
    wait_until(|| {
        app.widgets().sidebar_devices.rows.len() == 2 && !app.model().pane(PaneId::Left).loading
    });
    assert_eq!(
        eject_buttons(&app).len(),
        1,
        "System disk has no removal button"
    );
    assert!(
        eject_buttons(&app)[0]
            .tooltip_text()
            .unwrap()
            .contains("all volumes")
    );
    assert!(
        app.widgets().sidebar_devices.rows[0]
            .1
            .has_css_class("sidebar-row-active")
    );
    snapshot(&app, "devices-ready");

    *fake.failure.borrow_mut() =
        Some("USB Drive is busy. Close files and terminals using it, then try again.".to_owned());
    eject_buttons(&app)[0].emit_clicked();
    wait_until(|| app.model().devices.busy.is_some());
    assert!(
        app.widgets()
            .sidebar_devices
            .rows
            .iter()
            .all(|(_, button)| !button.is_sensitive())
    );
    app.emit(AppMsg::RemoveDevice("usb".to_owned()));
    wait_until(|| app.model().devices.busy.is_none());
    assert_eq!(
        fake.calls.get(),
        1,
        "Repeated clicks must not start another removal"
    );
    assert!(
        app.model()
            .devices
            .notice
            .as_ref()
            .unwrap()
            .contains("is busy")
    );
    assert_eq!(
        app.model().pane(PaneId::Left).current_directory(),
        &roots[0]
    );
    assert_eq!(
        eject_buttons(&app).len(),
        1,
        "Failure leaves the device available for retry"
    );
    snapshot(&app, "devices-busy");

    // Device disappearance between rendering and dispatch must never target a
    // different row in the list, or leave the UI in a loading state.
    app.emit(AppMsg::RemoveDevice("stale-key".to_owned()));
    wait_until(|| {
        app.model()
            .devices
            .notice
            .as_ref()
            .unwrap()
            .contains("no longer connected")
    });
    assert_eq!(fake.calls.get(), 1);

    let active = app.model().active_pane;
    app.state()
        .get_mut()
        .model
        .pane_mut(PaneId::Left)
        .tabs
        .push(TabState::new(VPath::from(usb.join("projects"))));
    eject_buttons(&app)[0].emit_clicked();
    wait_until(|| app.model().devices.busy.is_some());
    wait_until(|| app.model().devices.busy.is_none());
    assert_eq!(fake.calls.get(), 2);
    assert_eq!(app.model().active_pane, active);
    for pane in [PaneId::Left, PaneId::Right] {
        assert_eq!(app.model().pane(pane).current_directory(), &home_path());
        assert!(
            app.model()
                .pane(pane)
                .tabs
                .iter()
                .all(|tab| !under_roots(&tab.path, &roots))
        );
    }
    assert!(
        app.model()
            .devices
            .notice
            .as_ref()
            .unwrap()
            .contains("ejected")
    );
    assert!(eject_buttons(&app).is_empty());
    assert!(
        usb.is_dir() && second.is_dir(),
        "The fake backend must never remove real storage"
    );

    // Reconnecting an unmounted volume makes it mountable; the newly mounted row
    // becomes active without rebuilding the application or losing its actions.
    fake.entries.borrow_mut().push(DeviceEntry {
        key: "usb-again".to_owned(),
        name: "USB Drive".to_owned(),
        root: None,
        remote: false,
        removal: Some(Removal::Stop),
        removal_name: "USB Drive".to_owned(),
    });
    app.emit(AppMsg::DevicesChanged);
    wait_until(|| eject_buttons(&app).len() == 1);
    app.emit(AppMsg::OpenDevice("usb-again".to_owned()));
    wait_until(|| app.model().devices.busy.is_some());
    wait_until(|| app.model().devices.busy.is_none());
    assert_eq!(app.model().pane(active).current_directory(), &roots[0]);
    assert!(
        app.widgets()
            .sidebar_devices
            .rows
            .iter()
            .any(|(path, button)| path == &roots[0] && button.has_css_class("sidebar-row-active"))
    );
    fake.entries
        .borrow_mut()
        .retain(|entry| entry.key != "usb-again");
    app.emit(AppMsg::DeviceMountRemoved(roots[0].clone()));
    wait_until(|| app.model().pane(active).current_directory() == &home_path());
    assert!(eject_buttons(&app).is_empty());
    app.widget().close();
}

fn eject_buttons(app: &relm4::Controller<AppModel>) -> Vec<gtk::Button> {
    fn visit(widget: &gtk::Widget, buttons: &mut Vec<gtk::Button>) {
        if widget.has_css_class("sidebar-device-eject") {
            buttons.push(widget.clone().downcast().unwrap());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            visit(&widget, buttons);
        }
    }
    let mut buttons = Vec::new();
    visit(
        app.widgets().sidebar_devices.devices.upcast_ref(),
        &mut buttons,
    );
    buttons
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let context = glib::MainContext::default();
    while !condition() {
        assert!(Instant::now() < deadline, "device UI condition timed out");
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(app: &relm4::Controller<AppModel>, name: &str) {
    let Some(directory) = std::env::var_os("COMMANDER_DEVICE_SNAPSHOT_DIR") else {
        return;
    };
    let deadline = Instant::now() + Duration::from_millis(180);
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
    let widgets = app.widgets();
    let sidebar = &widgets.sidebar_revealer;
    let snapshot = gtk::Snapshot::new();
    sidebar.parent().unwrap().snapshot_child(sidebar, &snapshot);
    let node = snapshot.to_node().unwrap();
    app.widget()
        .renderer()
        .unwrap()
        .render_texture(&node, None)
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}
