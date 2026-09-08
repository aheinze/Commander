//! Archive locations keep native staging paths private to the file-operation layer.

use super::*;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub(crate) struct ArchiveMount {
    source: VPath,
    directory: tempfile::TempDir,
    size: u64,
    modified: Option<dualpane_core::Timestamp>,
}

#[derive(Default)]
pub(super) struct ArchiveLocations {
    mounts: Vec<Arc<ArchiveMount>>,
}

impl ArchiveLocations {
    pub(super) fn source_for(&self, path: &VPath) -> Option<VPath> {
        self.mounts
            .iter()
            .rev()
            .find(|mount| path.as_path().starts_with(mount.directory.path()))
            .map(|mount| mount.source.clone())
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
            if let Some(mount) = self
                .mounts
                .iter()
                .rev()
                .find(|mount| path.as_path().starts_with(mount.source.as_path()))
            {
                return join_relative(
                    mount.directory.path(),
                    path.as_path().strip_prefix(mount.source.as_path()).unwrap(),
                );
            }
            return path.clone();
        }
        let mut best = None;
        for mount in &self.mounts {
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

fn open_location(
    vfs: &dyn Vfs,
    target: &VPath,
    cached: &[Arc<ArchiveMount>],
    cancel: &CancelToken,
) -> Result<OpenedArchive, String> {
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
            let directory = tempfile::Builder::new()
                .prefix("commander-archive-")
                .tempdir()
                .map_err(|error| error.to_string())?;
            extract_archive(
                vfs,
                &source,
                &VPath::from(directory.path()),
                &mut ArchiveTask::new(cancel),
            )?;
            Arc::new(ArchiveMount {
                source,
                directory,
                size: metadata.size,
                modified: metadata.modified,
            })
        };
        let location = join_relative(mount.directory.path(), relative);
        return Ok(OpenedArchive { mount, location });
    }
    Err("No supported archive found at this location (ZIP, 7Z, TAR, TAR.GZ or TGZ)".to_owned())
}

impl AppModel {
    pub(super) fn on_archive_edited(&mut self, source: VPath, sender: &ComponentSender<Self>) {
        for pane in [PaneId::Left, PaneId::Right] {
            if self
                .archive_mounts
                .source_for(self.pane(pane).current_directory())
                .as_ref()
                == Some(&source)
            {
                // A removed subfolder may no longer exist. Reopen at the archive root.
                self.start_archive_location(pane, source.clone(), true, sender);
            } else {
                self.start_listing(pane, sender);
            }
        }
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
        let cached = self.archive_mounts.mounts.clone();
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("commander-archive-browser".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    open_location(vfs.as_ref(), &target, &cached, &cancel)
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
        let replace = self.pane(pane).archive_browse.replace_location;
        self.pane_mut(pane).archive_browse.cancel();
        match result {
            Ok(opened) => {
                if !self
                    .archive_mounts
                    .mounts
                    .iter()
                    .any(|mount| Arc::ptr_eq(mount, &opened.mount))
                {
                    // Tabs and history may still point into an earlier extracted copy.
                    // Rebase those references before exposing the replacement snapshot.
                    let previous: Vec<_> = self
                        .archive_mounts
                        .mounts
                        .iter()
                        .filter(|mount| mount.source == opened.mount.source)
                        .map(|mount| mount.directory.path().to_path_buf())
                        .collect();
                    for state in &mut self.panes {
                        for tab in &mut state.tabs {
                            for path in std::iter::once(&mut tab.path).chain(tab.history.iter_mut())
                            {
                                if let Some(relative) = previous
                                    .iter()
                                    .find_map(|root| path.as_path().strip_prefix(root).ok())
                                {
                                    let target = opened.mount.directory.path().join(relative);
                                    *path = VPath::from(if target.is_dir() {
                                        target
                                    } else {
                                        opened.mount.directory.path().to_path_buf()
                                    });
                                }
                            }
                        }
                        state.tabs_revision = state.tabs_revision.wrapping_add(1);
                    }
                    self.archive_mounts.mounts.push(opened.mount);
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
