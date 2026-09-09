//! Archive locations keep native staging paths private to the file-operation layer.

use super::*;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub(crate) struct ArchiveMount {
    pub(super) source: VPath,
    directory: tempfile::TempDir,
    password: Option<crate::archive::Password>,
    size: u64,
    modified: Option<dualpane_core::Timestamp>,
    pub(super) editable: Result<crate::archive::edit::Snapshot, String>,
}

impl ArchiveMount {
    pub(super) fn root(&self) -> &Path {
        self.directory.path()
    }

    pub(super) fn entry_name(&self, path: &VPath) -> Result<String, String> {
        let relative = path
            .as_path()
            .strip_prefix(self.root())
            .map_err(|_| "The archive location is no longer available")?;
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err("Invalid archive entry path".into());
        }
        relative
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| "Archive entries require UTF-8 file names".into())
    }
}

#[derive(Default)]
pub(super) struct ArchiveLocations {
    mounts: Vec<Arc<ArchiveMount>>,
    reloads: Vec<CancelToken>,
    superseded: BTreeSet<PathBuf>,
    reload_versions: HashMap<VPath, (JobId, CancelToken)>,
}

impl ArchiveLocations {
    pub(super) fn cancel_reloads(&self) {
        for cancel in &self.reloads {
            cancel.cancel();
        }
    }

    fn cached(&self) -> Vec<Arc<ArchiveMount>> {
        self.mounts
            .iter()
            .filter(|mount| !self.superseded.contains(mount.root()))
            .cloned()
            .collect()
    }

    pub(super) fn drop_roots(&self) -> Vec<(PathBuf, bool)> {
        self.mounts
            .iter()
            .map(|mount| (mount.root().to_path_buf(), mount.editable.is_ok()))
            .collect()
    }

    pub(super) fn mount_for(&self, path: &VPath) -> Option<Arc<ArchiveMount>> {
        self.mounts
            .iter()
            .rev()
            .find(|mount| path.as_path().starts_with(mount.root()))
            .cloned()
    }

    pub(super) fn read_only_reason(&self, path: &VPath) -> Option<&str> {
        self.mounts
            .iter()
            .rev()
            .find(|mount| path.as_path().starts_with(mount.root()))
            .and_then(|mount| mount.editable.as_ref().err().map(String::as_str))
    }
    pub(super) fn contains(&self, path: &VPath) -> bool {
        self.mounts
            .iter()
            .any(|mount| path.as_path().starts_with(mount.directory.path()))
    }

    pub(super) fn is_root(&self, path: &VPath) -> bool {
        self.mounts
            .iter()
            .any(|mount| path.as_path() == mount.directory.path())
    }

    pub(super) fn display(&self, path: &VPath) -> VPath {
        let mut result = path.clone();
        // Nested archives point back into an earlier mount; this bound also prevents cycles.
        for _ in 0..self.mounts.len() {
            let Some(mount) = self
                .mounts
                .iter()
                .rev()
                .find(|mount| result.as_path().starts_with(mount.directory.path()))
            else {
                break;
            };
            let relative = result
                .as_path()
                .strip_prefix(mount.directory.path())
                .unwrap();
            result = join_relative(mount.source.as_path(), relative);
        }
        result
    }

    pub(super) fn resolve(&self, path: &VPath) -> VPath {
        if self.contains(path) {
            if let Some(mount) = self.mounts.iter().rev().find(|mount| {
                !self.superseded.contains(mount.root())
                    && path.as_path().starts_with(mount.source.as_path())
            }) {
                return join_relative(
                    mount.directory.path(),
                    path.as_path().strip_prefix(mount.source.as_path()).unwrap(),
                );
            }
            return path.clone();
        }
        let mut best = None;
        for mount in self
            .mounts
            .iter()
            .filter(|mount| !self.superseded.contains(mount.root()))
        {
            let display = self.display(&VPath::from(mount.directory.path()));
            if let Ok(relative) = path.as_path().strip_prefix(display.as_path()) {
                let depth = display.as_path().components().count();
                if best.as_ref().is_none_or(|(length, _)| depth >= *length) {
                    // Resolve `archive.zip/../sibling` in the visible hierarchy.
                    if relative
                        .components()
                        .any(|part| part == Component::ParentDir)
                    {
                        return self.resolve(&VPath::from(normalize(path.as_path())));
                    }
                    best = Some((depth, join_relative(mount.directory.path(), relative)));
                }
            }
        }
        best.map_or_else(|| path.clone(), |(_, native)| native)
    }

    pub(super) fn parent(&self, path: &VPath) -> Option<VPath> {
        if self.is_root(path) {
            self.display(path)
                .parent()
                .map(|parent| self.resolve(&parent))
        } else {
            path.parent()
        }
    }

    pub(super) fn ancestors(&self, path: &VPath) -> Vec<VPath> {
        let mut result = Vec::new();
        let mut cursor = Some(path.clone());
        while let Some(path) = cursor {
            cursor = self.parent(&path).filter(|parent| parent != &path);
            result.push(path);
        }
        result.reverse();
        result
    }

    pub(super) fn session(&self, mut session: PaneSession) -> PaneSession {
        let display = |path: &str| self.display(&VPath::from(path)).to_string();
        session.tabs = session.tabs.iter().map(|path| display(path)).collect();
        session.folders = session
            .folders
            .into_iter()
            .map(|(path, view)| (display(&path), view))
            .collect();
        session.locations = session
            .locations
            .into_iter()
            .map(|(path, mut location)| {
                location.columns = location.columns.iter().map(|path| display(path)).collect();
                (display(&path), location)
            })
            .collect();
        session
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            _ => result.push(part),
        }
    }
    result
}

fn join_relative(root: &Path, relative: &Path) -> VPath {
    VPath::from(if relative.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    })
}

pub(super) fn has_archive_component(path: &VPath) -> bool {
    path.as_path()
        .ancestors()
        .any(|part| is_archive_path(&VPath::from(part)))
}

#[derive(Default)]
pub(super) struct ArchiveBrowseState {
    pub(super) source: Option<VPath>,
    id: Option<JobId>,
    cancel: Option<CancelToken>,
    replace_location: bool,
}

impl ArchiveBrowseState {
    pub(super) fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.id = None;
        self.source = None;
    }
}

#[derive(Debug)]
pub(crate) struct OpenedArchive {
    mount: Arc<ArchiveMount>,
    location: VPath,
}

#[cfg(test)]
fn open_location(
    vfs: &dyn Vfs,
    target: &VPath,
    cached: &[Arc<ArchiveMount>],
    cancel: &CancelToken,
) -> Result<OpenedArchive, String> {
    open_location_with_task(vfs, target, cached, &mut ArchiveTask::new(cancel))
}

fn open_location_with_task(
    vfs: &dyn Vfs,
    target: &VPath,
    cached: &[Arc<ArchiveMount>],
    task: &mut ArchiveTask<'_>,
) -> Result<OpenedArchive, String> {
    let cancel = task.cancel_token();
    let cancel = &cancel;
    for candidate in target.as_path().ancestors() {
        cancel
            .check()
            .map_err(|_| "Opening archive cancelled".to_owned())?;
        let source = VPath::from(candidate);
        if !is_archive_path(&source) {
            continue;
        }
        let Ok(metadata) = vfs.stat(&source, true) else {
            continue;
        };
        if metadata.kind != EntryKind::File {
            continue;
        }
        let relative = target.as_path().strip_prefix(candidate).unwrap();
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err("Use the Parent folder button to leave an archive".to_owned());
        }
        let mount = if let Some(mount) = cached.iter().rev().find(|mount| {
            mount.source == source
                && mount.size == metadata.size
                && mount.modified == metadata.modified
        }) {
            Arc::clone(mount)
        } else {
            if let Some(previous) = cached.iter().rev().find(|mount| mount.source == source) {
                task.set_password(previous.password.clone());
            }
            let directory = tempfile::Builder::new()
                .prefix("commander-archive-")
                .tempdir()
                .map_err(|error| error.to_string())?;
            let original_digest = crate::features::sha256(vfs, &source, cancel)?;
            extract_archive(vfs, &source, &VPath::from(directory.path()), task)?;
            let editable = if cached
                .iter()
                .any(|mount| source.as_path().starts_with(mount.root()))
            {
                Err(
                    "Nested archives are read-only. Copy this archive out to change its contents."
                        .into(),
                )
            } else if metadata.mode.is_some_and(|mode| mode & 0o222 == 0) {
                Err("The archive file is read-only.".into())
            } else {
                crate::archive::edit::inspect_with_password(&source, cancel, task.password())
            };
            if let Ok(snapshot) = &editable
                && original_digest != snapshot.digest
            {
                return Err("Archive changed while opening. Open it again.".into());
            }
            Arc::new(ArchiveMount {
                source,
                password: task.password(),
                directory,
                size: metadata.size,
                modified: metadata.modified,
                editable,
            })
        };
        let location = join_relative(mount.directory.path(), relative);
        return Ok(OpenedArchive { mount, location });
    }
    Err("No supported archive found at this location (ZIP, 7Z, TAR, TAR.GZ or TGZ)".to_owned())
}

impl AppModel {
    pub(super) fn archive_browse_is_current(&self, pane: PaneId, id: JobId) -> bool {
        self.pane(pane).archive_browse.id == Some(id)
    }

    pub(super) fn archive_reload_is_current(&self, source: &VPath, id: JobId) -> bool {
        self.archive_mounts
            .reload_versions
            .get(source)
            .is_some_and(|(current, _)| *current == id)
    }

    pub(super) fn on_archive_edited(&mut self, source: VPath, sender: &ComponentSender<Self>) {
        let id = JobId::next();
        let cancel = CancelToken::new();
        if let Some((_, previous)) = self
            .archive_mounts
            .reload_versions
            .insert(source.clone(), (id, cancel.clone()))
        {
            previous.cancel();
        }
        let vfs = Arc::clone(&self.vfs);
        let cached = self.archive_mounts.cached();
        let input = sender.input_sender().clone();
        self.archive_mounts
            .reloads
            .retain(|cancel| !cancel.is_cancelled());
        self.archive_mounts.reloads.push(cancel.clone());
        match thread::Builder::new()
            .name("commander-archive-refresh".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut task = ArchiveTask::new(&cancel);
                    task.set_password_prompt(|source, incorrect| {
                        archive_password::request(
                            &input,
                            archive_password::Context::Reload(id),
                            source,
                            incorrect,
                            &cancel,
                        )
                    });
                    open_location_with_task(vfs.as_ref(), &source, &cached, &mut task)
                }))
                .unwrap_or_else(|_| Err("The archive reader stopped unexpectedly".into()));
                cancel.cancel();
                let _ = input.send(AppMsg::ArchiveReloadReady { source, id, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => notifications::error(&format!("Could not refresh archive: {error}")),
        }
    }

    pub(super) fn on_archive_reload_ready(
        &mut self,
        source: VPath,
        id: JobId,
        result: Result<OpenedArchive, String>,
        sender: &ComponentSender<Self>,
    ) {
        if !self.archive_reload_is_current(&source, id) {
            return;
        }
        match result {
            Ok(opened) => {
                self.install_archive_mount(opened.mount);
                for pane in [PaneId::Left, PaneId::Right] {
                    // Reset columns as well as the list after rebasing an archive tab.
                    if self
                        .archive_mounts
                        .contains(self.pane(pane).current_directory())
                    {
                        self.pane_mut(pane).reset_directory_view();
                    }
                    self.start_listing(pane, sender);
                }
                self.refresh_search(sender);
                self.persist_session();
            }
            Err(error) => notifications::error(&format!("Could not refresh archive: {error}")),
        }
    }

    fn install_archive_mount(&mut self, mount: Arc<ArchiveMount>) {
        if self
            .archive_mounts
            .mounts
            .iter()
            .any(|old| Arc::ptr_eq(old, &mount))
        {
            return;
        }
        let previous: Vec<_> = self
            .archive_mounts
            .mounts
            .iter()
            .filter(|old| old.source == mount.source)
            .map(|old| old.root().to_path_buf())
            .collect();
        let visible_source = self.archive_mounts.display(&mount.source);
        let nested: Vec<_> = self
            .archive_mounts
            .mounts
            .iter()
            .filter(|old| old.source != mount.source)
            .filter_map(|old| {
                let visible = self.archive_mounts.display(&VPath::from(old.root()));
                visible
                    .as_path()
                    .strip_prefix(visible_source.as_path())
                    .ok()
                    .map(|_| old.root().to_path_buf())
            })
            .collect();
        let rebase = |path: &mut VPath| {
            if let Some(relative) = previous
                .iter()
                .find_map(|root| path.as_path().strip_prefix(root).ok())
            {
                let target = mount.root().join(relative);
                *path = VPath::from(if target.is_dir() {
                    target
                } else {
                    mount.root().to_path_buf()
                });
            } else if nested.iter().any(|root| path.as_path().starts_with(root)) {
                // A changed outer archive invalidates any extracted nested snapshot.
                *path = VPath::from(mount.root());
            }
        };
        if let Some(session) = &mut self.search_session {
            rebase(&mut session.root);
        }
        for state in &mut self.panes {
            for tab in &mut state.tabs {
                for path in std::iter::once(&mut tab.path).chain(tab.history.iter_mut()) {
                    rebase(path);
                }
            }
            state.tabs_revision = state.tabs_revision.wrapping_add(1);
        }
        // Keep retired snapshots alive for copies already reading their files.
        // They no longer participate in resolving visible archive locations.
        self.archive_mounts.superseded.extend(nested);
        self.archive_mounts.mounts.push(mount);
    }

    pub(super) fn is_archive_browse_path(&self, path: &VPath) -> bool {
        self.archive_mounts.contains(path)
    }

    pub(super) fn start_archive_browse(
        &mut self,
        pane: PaneId,
        source: VPath,
        sender: &ComponentSender<Self>,
    ) {
        self.start_archive_location(pane, source, false, sender);
    }

    pub(super) fn start_archive_location(
        &mut self,
        pane: PaneId,
        target: VPath,
        replace_location: bool,
        sender: &ComponentSender<Self>,
    ) {
        self.pane_mut(pane).archive_browse.cancel();
        let id = JobId::next();
        let cancel = CancelToken::new();
        let display = self.archive_mounts.display(&target);
        let source = display
            .as_path()
            .ancestors()
            .map(VPath::from)
            .find(is_archive_path)
            .unwrap_or(display);
        self.pane_mut(pane).archive_browse = ArchiveBrowseState {
            source: Some(source),
            id: Some(id),
            cancel: Some(cancel.clone()),
            replace_location,
        };
        self.pane_mut(pane).error = None;
        self.pane_mut(pane).loading = false;
        let vfs = Arc::clone(&self.vfs);
        let cached = self.archive_mounts.cached();
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("commander-archive-browser".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut task = ArchiveTask::new(&cancel);
                    task.set_password_prompt(|source, incorrect| {
                        archive_password::request(
                            &input,
                            archive_password::Context::Browse(pane, id),
                            source,
                            incorrect,
                            &cancel,
                        )
                    });
                    open_location_with_task(vfs.as_ref(), &target, &cached, &mut task)
                }))
                .unwrap_or_else(|_| Err("The archive reader stopped unexpectedly".to_owned()));
                let _ = input.send(AppMsg::ArchiveBrowseReady { pane, id, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.pane_mut(pane).archive_browse.cancel();
                self.pane_mut(pane).error = Some(format!("Could not open archive: {error}"));
            }
        }
    }

    pub(super) fn on_archive_browse_ready(
        &mut self,
        pane: PaneId,
        id: JobId,
        result: Result<OpenedArchive, String>,
        sender: &ComponentSender<Self>,
    ) {
        if self.pane(pane).archive_browse.id != Some(id) {
            return;
        }
        if self
            .pane(pane)
            .archive_browse
            .cancel
            .as_ref()
            .is_some_and(CancelToken::is_cancelled)
        {
            self.cancel_archive_open(pane, sender);
            return;
        }
        let replace = self.pane(pane).archive_browse.replace_location;
        self.pane_mut(pane).archive_browse.cancel();
        match result {
            Ok(mut opened) => {
                // Concurrent pane openings can finish with equivalent snapshots.
                if let Some(current) = self.archive_mounts.mounts.iter().rev().find(|mount| {
                    mount.source == opened.mount.source
                        && mount.size == opened.mount.size
                        && mount.modified == opened.mount.modified
                }) {
                    opened.location = join_relative(
                        current.root(),
                        opened
                            .location
                            .as_path()
                            .strip_prefix(opened.mount.root())
                            .unwrap(),
                    );
                    opened.mount = Arc::clone(current);
                }
                if replace
                    && !opened.location.as_path().is_dir()
                    && !has_archive_component(&opened.location)
                {
                    opened.location = VPath::from(opened.mount.root());
                }
                let other = if pane == PaneId::Left {
                    PaneId::Right
                } else {
                    PaneId::Left
                };
                let other_path = self.pane(other).active().path.clone();
                self.install_archive_mount(opened.mount);
                if self.pane(other).active().path != other_path {
                    self.pane_mut(other).reset_directory_view();
                    self.start_listing(other, sender);
                }
                self.resolve_saved_archive_navigation(pane);
                if replace {
                    self.replace_archive_location(pane, opened.location);
                    self.start_listing(pane, sender);
                    self.persist_session();
                } else {
                    let active = self.active_pane;
                    self.navigate_exact(pane, opened.location, sender);
                    if active != pane {
                        self.active_pane = active;
                        self.focus_active_files();
                    }
                }
            }
            Err(error) => {
                self.pane_mut(pane).error = Some(format!("Could not open archive: {error}"))
            }
        }
    }

    pub(super) fn cancel_archive_open(&mut self, pane: PaneId, sender: &ComponentSender<Self>) {
        let state = &self.pane(pane).archive_browse;
        let parent = state
            .replace_location
            .then(|| state.source.as_ref().and_then(VPath::parent))
            .flatten();
        self.pane_mut(pane).archive_browse.cancel();
        if let Some(parent) = parent {
            if self.pane(pane).active().history_index > 0 {
                self.on_back(pane, sender);
            } else {
                self.navigate_exact(pane, parent, sender);
            }
        }
    }

    pub(super) fn replace_archive_location(&mut self, pane: PaneId, location: VPath) {
        let state = self.pane_mut(pane);
        let tab = state.active_mut();
        tab.history[tab.history_index] = location.clone();
        tab.path = location;
        state.reset_directory_view();
        state.tabs_revision = state.tabs_revision.wrapping_add(1);
    }

    fn resolve_saved_archive_navigation(&mut self, pane: PaneId) {
        let index = pane.index();
        self.panes[index].folder_views = std::mem::take(&mut self.panes[index].folder_views)
            .into_iter()
            .map(|(path, view)| {
                (
                    self.archive_mounts
                        .resolve(&VPath::from(path.as_str()))
                        .to_string(),
                    view,
                )
            })
            .collect();
        self.panes[index].locations = std::mem::take(&mut self.panes[index].locations)
            .into_iter()
            .map(|(path, mut location)| {
                location.columns = location
                    .columns
                    .iter()
                    .map(|path| {
                        self.archive_mounts
                            .resolve(&VPath::from(path.as_str()))
                            .to_string()
                    })
                    .collect();
                (
                    self.archive_mounts
                        .resolve(&VPath::from(path.as_str()))
                        .to_string(),
                    location,
                )
            })
            .collect();
    }
}

#[cfg(test)]
mod tests;
