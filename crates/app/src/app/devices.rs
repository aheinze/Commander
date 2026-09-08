//! Live GIO devices and safe, asynchronous removal. No forced unmounts.

use super::*;
use std::{future::Future, pin::Pin};

type DeviceFuture = Pin<Box<dyn Future<Output = Result<DeviceOutcome, String>>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Removal {
    Stop,
    Eject,
    Unmount,
}

impl Removal {
    fn label(self) -> &'static str {
        match self {
            Self::Stop => "Safely Remove",
            Self::Eject => "Eject",
            Self::Unmount => "Unmount",
        }
    }
}

#[derive(Clone, Debug)]
struct DeviceEntry {
    key: String,
    name: String,
    root: Option<VPath>,
    uri: Option<String>,
    remote: bool,
    removal: Option<Removal>,
    removal_name: String,
}

#[derive(Debug)]
pub enum DeviceOutcome {
    Open(VPath),
    Removed { roots: Vec<VPath>, message: String },
}

struct PreparedAction {
    roots: Vec<VPath>,
    progress: String,
    future: DeviceFuture,
}

// The backend boundary lets regression tests exercise UI and completion paths
// without mounting, ejecting, or otherwise touching the user's actual drives.
trait DeviceBackend {
    fn entries(&self) -> Vec<DeviceEntry>;
    fn prepare(&self, key: &str, remove: bool) -> Result<PreparedAction, String>;
}

pub(super) struct DeviceState {
    backend: Rc<dyn DeviceBackend>,
    entries: Vec<DeviceEntry>,
    revision: u64,
    busy: Option<String>,
    notice: Option<String>,
}

pub(super) struct RemoteMount {
    pub key: String,
    pub destination: VPath,
    pub removable: bool,
    pub busy: bool,
}

impl DeviceState {
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn mounted_remote(&self, uri: &str) -> Option<RemoteMount> {
        self.entries
            .iter()
            .filter(|entry| entry.remote)
            .filter_map(|entry| {
                let relative = remote::relative_mount_path(entry.uri.as_deref()?, uri)?;
                let root = entry.root.as_ref()?;
                Some((
                    relative.components().count(),
                    RemoteMount {
                        key: entry.key.clone(),
                        destination: VPath::from(root.as_path().join(relative)),
                        removable: entry.removal.is_some(),
                        busy: self.busy.is_some(),
                    },
                ))
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, mount)| mount)
    }

    pub(super) fn new() -> Self {
        let backend: Rc<dyn DeviceBackend> = Rc::new(GioDevices);
        Self {
            entries: backend.entries(),
            backend,
            revision: 0,
            busy: None,
            notice: None,
        }
    }

    pub(super) fn refresh(&mut self) {
        self.entries = self.backend.entries();
        self.revision = self.revision.wrapping_add(1);
    }
}

enum Target {
    Volume(gio::Volume),
    Mount(gio::Mount),
}

enum RemoveTarget {
    Stop(gio::Drive),
    Drive(gio::Drive),
    Volume(gio::Volume),
    Mount(gio::Mount),
    Unmount(gio::Mount),
}

impl Target {
    fn mount(&self) -> Option<gio::Mount> {
        match self {
            Self::Volume(volume) => volume.get_mount(),
            Self::Mount(mount) => Some(mount.clone()),
        }
    }

    fn removal(&self) -> Option<RemoveTarget> {
        let mount = self.mount();
        let drive = match self {
            Self::Volume(volume) => volume.drive(),
            Self::Mount(mount) => mount.drive(),
        };
        if let Some(drive) = drive {
            if drive.can_stop() {
                return Some(RemoveTarget::Stop(drive));
            }
            if drive.can_eject() {
                return Some(RemoveTarget::Drive(drive));
            }
        }
        if let Self::Volume(volume) = self
            && volume.can_eject()
        {
            return Some(RemoveTarget::Volume(volume.clone()));
        }
        mount.and_then(|mount| {
            if mount.can_eject() {
                Some(RemoveTarget::Mount(mount))
            } else if mount.can_unmount() {
                Some(RemoveTarget::Unmount(mount))
            } else {
                None
            }
        })
    }

    fn entry(&self) -> DeviceEntry {
        let (key, name, remote) = match self {
            // Object identity remains stable across monitor snapshots, including
            // devices without UUIDs and drives with duplicated filesystem UUIDs.
            Self::Volume(volume) => (
                format!("volume:{:p}", volume.as_ptr()),
                volume.name().to_string(),
                false,
            ),
            Self::Mount(mount) => (
                format!("mount:{:p}", mount.as_ptr()),
                mount.name().to_string(),
                true,
            ),
        };
        let removal = self.removal();
        let removal_name = match &removal {
            Some(RemoveTarget::Stop(drive) | RemoveTarget::Drive(drive))
                if drive.volumes().len() > 1 =>
            {
                format!("{} (all volumes)", drive.name())
            }
            _ => name.clone(),
        };
        DeviceEntry {
            key,
            name,
            root: self
                .mount()
                .and_then(|mount| mount.root().path())
                .map(VPath::from),
            uri: self.mount().map(|mount| mount.root().uri().to_string()),
            remote,
            removal: removal.as_ref().map(RemoveTarget::kind),
            removal_name,
        }
    }
}

impl RemoveTarget {
    fn kind(&self) -> Removal {
        match self {
            Self::Stop(_) => Removal::Stop,
            Self::Unmount(_) => Removal::Unmount,
            _ => Removal::Eject,
        }
    }

    fn roots(&self) -> Vec<VPath> {
        let mounts: Vec<gio::Mount> = match self {
            Self::Stop(drive) | Self::Drive(drive) => drive
                .volumes()
                .iter()
                .filter_map(|volume| volume.get_mount())
                .collect(),
            Self::Volume(volume) => volume.get_mount().into_iter().collect(),
            Self::Mount(mount) | Self::Unmount(mount) => vec![mount.clone()],
        };
        mounts
            .iter()
            .filter_map(|mount| mount.root().path())
            .map(VPath::from)
            .collect()
    }

    async fn run(self, operation: &gio::MountOperation) -> Result<(), glib::Error> {
        let flags = gio::MountUnmountFlags::NONE;
        match self {
            Self::Stop(drive) => drive.stop_future(flags, Some(operation)).await,
            Self::Drive(drive) => {
                drive
                    .eject_with_operation_future(flags, Some(operation))
                    .await
            }
            Self::Volume(volume) => {
                volume
                    .eject_with_operation_future(flags, Some(operation))
                    .await
            }
            Self::Mount(mount) => {
                mount
                    .eject_with_operation_future(flags, Some(operation))
                    .await
            }
            Self::Unmount(mount) => {
                mount
                    .unmount_with_operation_future(flags, Some(operation))
                    .await
            }
        }
    }
}

struct GioDevices;

impl GioDevices {
    fn targets() -> Vec<Target> {
        let monitor = gio::VolumeMonitor::get();
        let mut targets: Vec<_> = monitor.volumes().into_iter().map(Target::Volume).collect();
        let mounted: HashSet<_> = targets
            .iter()
            .filter_map(Target::mount)
            .map(|mount| mount.root().uri())
            .collect();
        targets.extend(
            monitor
                .mounts()
                .into_iter()
                .filter(|mount| !mount.is_shadowed() && !mounted.contains(&mount.root().uri()))
                .filter(|mount| mount.root().path().is_some())
                .map(Target::Mount),
        );
        targets
    }
}

impl DeviceBackend for GioDevices {
    fn entries(&self) -> Vec<DeviceEntry> {
        Self::targets().iter().map(Target::entry).collect()
    }

    fn prepare(&self, key: &str, remove: bool) -> Result<PreparedAction, String> {
        let target = Self::targets()
            .into_iter()
            .find(|target| target.entry().key == key)
            .ok_or("This device is no longer connected.")?;
        let entry = target.entry();
        if !remove {
            return Ok(PreparedAction {
                roots: Vec::new(),
                progress: format!("Opening {}…", entry.name),
                future: Box::pin(async move {
                    if let Some(root) = entry.root {
                        return Ok(DeviceOutcome::Open(root));
                    }
                    let Target::Volume(volume) = target else {
                        return Err("This location cannot be opened as a folder.".to_owned());
                    };
                    let operation = super::remote::mount_operation();
                    volume
                        .mount_future(gio::MountMountFlags::NONE, Some(&operation))
                        .await
                        .map_err(|error| format!("Could not mount {}: {error}", entry.name))?;
                    volume
                        .get_mount()
                        .and_then(|mount| mount.root().path())
                        .map(|path| DeviceOutcome::Open(VPath::from(path)))
                        .ok_or_else(|| {
                            "The device mounted without an accessible folder.".to_owned()
                        })
                }),
            });
        }
        let removal = target
            .removal()
            .ok_or("This device does not support removal.")?;
        let roots = removal.roots();
        let removed_roots = roots.clone();
        let kind = removal.kind();
        Ok(PreparedAction {
            roots,
            progress: format!(
                "{} {}…",
                if kind == Removal::Unmount {
                    "Unmounting"
                } else {
                    "Ejecting"
                },
                entry.removal_name
            ),
            future: Box::pin(async move {
                let operation = super::remote::mount_operation();
                let busy = Rc::new(Cell::new(false));
                let blocked = busy.clone();
                operation.connect_local("show-processes", false, move |values| {
                    if let Ok(operation) = values[0].get::<gio::MountOperation>() {
                        blocked.set(true);
                        operation.stop_signal_emission_by_name("show-processes");
                        operation.reply(gio::MountOperationResult::Aborted);
                    }
                    None
                });
                removal
                    .run(&operation)
                    .await
                    .map_err(|error| removal_error(&entry.removal_name, &error, busy.get()))?;
                let message = match kind {
                    Removal::Unmount => format!("{} unmounted.", entry.name),
                    _ => format!("{} ejected. You can disconnect it.", entry.removal_name),
                };
                Ok(DeviceOutcome::Removed {
                    roots: removed_roots,
                    message,
                })
            }),
        })
    }
}

fn removal_error(name: &str, error: &glib::Error, blocked: bool) -> String {
    if blocked || error.matches(gio::IOErrorEnum::Busy) {
        format!("{name} is busy. Close files and terminals using it, then try again.")
    } else if error.matches(gio::IOErrorEnum::Cancelled)
        || error.matches(gio::IOErrorEnum::FailedHandled)
    {
        format!("Removal of {name} was canceled.")
    } else {
        format!("Could not remove {name}: {error}")
    }
}

fn under_roots(path: &VPath, roots: &[VPath]) -> bool {
    roots
        .iter()
        .any(|root| path.as_path().starts_with(root.as_path()))
}

fn operation_uses_roots(retry: &OperationRetry, roots: &[VPath]) -> bool {
    let (sources, destination) = match retry {
        OperationRetry::Copy {
            sources,
            destination,
            ..
        }
        | OperationRetry::Move {
            sources,
            destination,
            ..
        }
        | OperationRetry::Archive {
            sources,
            destination,
            ..
        } => (sources, Some(destination)),
        OperationRetry::Trash { sources, .. }
        | OperationRetry::Delete { sources, .. }
        | OperationRetry::SecureDelete { sources } => (sources, None),
    };
    sources.iter().any(|path| under_roots(path, roots))
        || destination.is_some_and(|path| under_roots(path, roots))
}

impl AppModel {
    pub(super) fn start_device_action(
        &mut self,
        key: &str,
        remove: bool,
        sender: &ComponentSender<Self>,
    ) {
        if self.devices.busy.is_some() {
            return;
        }
        let prepared = self.devices.backend.prepare(key, remove);
        let prepared = match prepared {
            Ok(action) => action,
            Err(error) => {
                notifications::error(&error);
                self.devices.notice = Some(error);
                self.devices.refresh();
                return;
            }
        };
        if remove
            && (self.history_busy
                || self.operations.values().any(|operation| {
                    !matches!(
                        operation.state,
                        JobState::Done | JobState::Cancelled | JobState::Failed
                    ) && operation_uses_roots(&operation.retry, &prepared.roots)
                }))
        {
            self.devices.notice = Some(
                "Finish or cancel the file operation using this device before removing it."
                    .to_owned(),
            );
            notifications::error(self.devices.notice.as_deref().unwrap());
            self.devices.refresh();
            return;
        }
        self.devices.busy = Some(key.to_owned());
        notifications::info(&prepared.progress);
        self.devices.notice = Some(prepared.progress);
        self.devices.refresh();
        let input = sender.input_sender().clone();
        glib::spawn_future_local(async move {
            let result = prepared.future.await;
            let _ = input.send(AppMsg::DeviceFinished(result));
        });
    }

    pub(super) fn finish_device_action(
        &mut self,
        result: Result<DeviceOutcome, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.devices.busy = None;
        match result {
            Ok(DeviceOutcome::Open(path)) => {
                self.devices.notice = None;
                self.navigate(self.active_pane, path, sender);
            }
            Ok(DeviceOutcome::Removed { roots, message }) => {
                notifications::success(&message);
                self.leave_device_paths(&roots, sender);
                self.push_operation_log(message.clone());
                self.devices.notice = Some(message);
            }
            Err(error) => {
                self.push_operation_log(error.clone());
                notifications::error(&error);
                self.devices.notice = Some(error);
            }
        }
        self.devices.refresh();
    }

    pub(super) fn leave_device_paths(&mut self, roots: &[VPath], sender: &ComponentSender<Self>) {
        let home = home_path();
        let fallback = if under_roots(&home, roots) {
            VPath::from(std::path::Path::new("/"))
        } else {
            home
        };
        let mut changed = false;
        for pane in [PaneId::Left, PaneId::Right] {
            let state = self.pane_mut(pane);
            let active_removed = under_roots(state.current_directory(), roots);
            let mut tabs_changed = false;
            for (index, tab) in state.tabs.iter_mut().enumerate() {
                if under_roots(&tab.path, roots) || (index == state.active_tab && active_removed) {
                    tab.navigate(fallback.clone());
                    tabs_changed = true;
                }
            }
            if tabs_changed {
                state.tabs_revision = state.tabs_revision.wrapping_add(1);
                changed = true;
            }
            if active_removed {
                state.reset_directory_view();
                self.start_listing(pane, sender);
            }
        }
        if changed {
            self.persist_session();
        }
    }
}

pub(super) struct DeviceSidebar {
    pub(super) devices: gtk::Box,
    pub(super) mounts: gtk::Box,
    pub(super) rows: Vec<(VPath, gtk::Button)>,
    monitor: gio::VolumeMonitor,
    signals: Vec<glib::SignalHandlerId>,
    rendered: Option<u64>,
    rendered_remotes: Vec<String>,
}

impl Drop for DeviceSidebar {
    fn drop(&mut self) {
        for signal in self.signals.drain(..) {
            self.monitor.disconnect(signal);
        }
    }
}

impl DeviceSidebar {
    pub(super) fn new(sender: &ComponentSender<AppModel>) -> Self {
        let monitor = gio::VolumeMonitor::get();
        let mut signals = Vec::new();
        for signal in [
            "volume-added",
            "volume-removed",
            "volume-changed",
            "mount-added",
            "mount-changed",
            "drive-connected",
            "drive-disconnected",
            "drive-changed",
        ] {
            let input = sender.input_sender().clone();
            signals.push(monitor.connect_local(signal, false, move |_| {
                let _ = input.send(AppMsg::DevicesChanged);
                None
            }));
        }
        let input = sender.input_sender().clone();
        signals.push(monitor.connect_mount_removed(move |_, mount| {
            let message = mount.root().path().map_or(AppMsg::DevicesChanged, |path| {
                AppMsg::DeviceMountRemoved(VPath::from(path))
            });
            let _ = input.send(message);
        }));
        Self {
            devices: gtk::Box::new(gtk::Orientation::Vertical, 2),
            mounts: gtk::Box::new(gtk::Orientation::Vertical, 2),
            rows: Vec::new(),
            monitor,
            signals,
            rendered: None,
            rendered_remotes: Vec::new(),
        }
    }

    pub(super) fn render(
        &mut self,
        state: &DeviceState,
        remotes: &[String],
        sender: &ComponentSender<AppModel>,
        file_drag_ui: &Rc<FileDragUiState>,
    ) -> bool {
        if self.rendered == Some(state.revision) && self.rendered_remotes == remotes {
            return false;
        }
        self.rendered = Some(state.revision);
        self.rendered_remotes = remotes.to_vec();
        self.rows.clear();
        for container in [&self.devices, &self.mounts] {
            while let Some(child) = container.first_child() {
                super::widgets::remove_sidebar_popovers(&child);
                container.remove(&child);
            }
        }
        for entry in &state.entries {
            if entry.remote
                && remotes.iter().any(|uri| {
                    state
                        .mounted_remote(uri)
                        .is_some_and(|mount| mount.key == entry.key)
                })
            {
                continue;
            }
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            let button = sidebar_button(
                &entry.name,
                if entry.remote {
                    "commander-server-symbolic"
                } else {
                    "commander-hard-drive-symbolic"
                },
            );
            button.set_hexpand(true);
            button.set_sensitive(state.busy.is_none());
            if let Some(path) = &entry.root {
                button.set_tooltip_text(Some(&path.to_string()));
                self.rows.push((path.clone(), button.clone()));
                let path = path.clone();
                if state.busy.is_none() {
                    install_file_drop_target(&button, sender, file_drag_ui.clone(), move |_| {
                        Some(path.clone())
                    });
                }
            } else {
                button.add_css_class("sidebar-row-unmounted");
                button.set_tooltip_text(Some("Mount and open"));
            }
            let key = entry.key.clone();
            let input = sender.input_sender().clone();
            button.connect_clicked(move |_| {
                let _ = input.send(AppMsg::OpenDevice(key.clone()));
            });
            row.append(&button);
            let (menu, actions) = super::sidebar::menus::new(&button, &entry.name);
            if let Some(path) = &entry.root {
                super::sidebar::menus::location_actions(
                    &menu,
                    &actions,
                    path,
                    true,
                    sender.input_sender(),
                );
            } else {
                let key = entry.key.clone();
                super::sidebar::menus::message(
                    &menu,
                    &actions,
                    "Mount and open",
                    "commander-folder-open-symbolic",
                    sender.input_sender(),
                    move || AppMsg::OpenDevice(key.clone()),
                );
            }
            if let Some(removal) = entry.removal {
                super::sidebar::menus::separator(&actions);
                let key = entry.key.clone();
                super::sidebar::menus::message(
                    &menu,
                    &actions,
                    if entry.remote {
                        "Disconnect"
                    } else {
                        removal.label()
                    },
                    "commander-eject-symbolic",
                    sender.input_sender(),
                    move || AppMsg::RemoveDevice(key.clone()),
                );
            }
            super::sidebar::menus::install(&button, &menu);
            actions.set_sensitive(state.busy.is_none());
            if state.busy.as_deref() == Some(&entry.key) {
                let spinner = gtk::Spinner::new();
                spinner.set_size_request(28, 28);
                spinner.set_spinning(true);
                row.append(&spinner);
            } else if let Some(action) = entry.removal {
                let label = format!("{} {}", action.label(), entry.removal_name);
                let eject = icon_button("commander-eject-symbolic", &label);
                eject.add_css_class("flat");
                eject.add_css_class("sidebar-device-eject");
                eject.set_valign(gtk::Align::Center);
                eject.set_sensitive(state.busy.is_none());
                eject.update_property(&[gtk::accessible::Property::Label(&label)]);
                let key = entry.key.clone();
                let input = sender.input_sender().clone();
                eject.connect_clicked(move |_| {
                    let _ = input.send(AppMsg::RemoveDevice(key.clone()));
                });
                row.append(&eject);
            }
            if entry.remote {
                self.mounts.append(&row);
            } else {
                self.devices.append(&row);
            }
        }
        for (container, remote, message) in [
            (&self.devices, false, "No devices connected"),
            (&self.mounts, true, "No mounted locations"),
        ] {
            if !state.entries.iter().any(|entry| entry.remote == remote)
                && (!remote || remotes.is_empty())
            {
                let empty = gtk::Label::new(Some(message));
                empty.add_css_class("sidebar-empty");
                empty.set_xalign(0.0);
                container.append(&empty);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests;
