//! Quiet, asynchronous Git context for the directory displayed in each pane.

use super::*;
use std::io::{BufRead, BufReader};
use std::process::Stdio;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GitStatus {
    branch: String,
    oid: String,
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    changed: bool,
    conflicts: bool,
}

impl GitStatus {
    fn branch_label(&self) -> String {
        if self.branch == "(detached)" {
            format!("HEAD · {}", self.oid.chars().take(7).collect::<String>())
        } else {
            self.branch.clone()
        }
    }

    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.conflicts {
            parts.push("!".to_owned());
        } else if self.changed {
            parts.push("*".to_owned());
        }
        if self.ahead > 0 {
            parts.push(format!("↑{}", self.ahead));
        }
        if self.behind > 0 {
            parts.push(format!("↓{}", self.behind));
        }
        parts.join(" ")
    }

    fn tooltip(&self) -> String {
        let mut lines = vec![if self.branch == "(detached)" {
            format!("Git · Detached HEAD at {}", self.oid)
        } else {
            format!("Git · {}", self.branch)
        }];
        if self.oid == "(initial)" {
            lines.push("No commits yet".to_owned());
        }
        lines.push(
            if self.conflicts {
                "Working tree has merge conflicts (!)"
            } else if self.changed {
                "Working tree has uncommitted changes (*)"
            } else {
                "Working tree clean"
            }
            .to_owned(),
        );
        if let Some(upstream) = &self.upstream {
            lines.push(format!("Tracking {upstream}"));
            lines.push(if self.ahead == 0 && self.behind == 0 {
                "Up to date with the locally known upstream".to_owned()
            } else {
                format!(
                    "{} ahead, {} behind (locally known upstream)",
                    self.ahead, self.behind
                )
            });
        }
        lines.join("\n")
    }
}

#[derive(Default)]
pub(super) struct PaneGitState {
    path: Option<VPath>,
    pub(super) info: Option<GitStatus>,
    checked: Option<Instant>,
    inflight: Option<(VPath, CancelToken)>,
}

impl PaneGitState {
    fn prepare(&mut self, path: Option<VPath>) {
        if self.path != path {
            self.cancel();
            self.path = path;
            self.info = None;
            self.checked = None;
        }
    }

    pub(super) fn cancel(&self) {
        if let Some((_, cancel)) = &self.inflight {
            cancel.cancel();
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.checked = None;
    }

    fn finish(&mut self, path: &VPath, info: Option<GitStatus>) {
        let Some((requested, cancel)) = self.inflight.take() else {
            return;
        };
        if &requested == path && self.path.as_ref() == Some(path) && !cancel.is_cancelled() {
            self.info = info;
        }
    }
}

impl AppModel {
    pub(super) fn sync_pane_git(&mut self, sender: &ComponentSender<Self>) {
        for pane in [PaneId::Left, PaneId::Right] {
            let directory = self.pane(pane).current_directory();
            let visible = self.dual_pane || self.active_pane == pane;
            let path =
                (visible && !self.is_archive_browse_path(directory)).then(|| directory.clone());
            let state = &mut self.pane_mut(pane).git;
            state.prepare(path);
            if state.inflight.is_some()
                || state
                    .checked
                    .is_some_and(|time| time.elapsed() < REFRESH_INTERVAL)
            {
                continue;
            }
            let Some(path) = state.path.clone() else {
                continue;
            };
            let cancel = CancelToken::new();
            state.checked = Some(Instant::now());
            state.inflight = Some((path.clone(), cancel.clone()));
            let input = sender.input_sender().clone();
            match thread::Builder::new()
                .name("commander-pane-git".to_owned())
                .spawn(move || {
                    let info = read_status(&path, &cancel);
                    let _ = input.send(AppMsg::PaneGitReady { pane, path, info });
                }) {
                Ok(worker) => self.aux_workers.push(worker),
                Err(_) => self.pane_mut(pane).git.inflight = None,
            }
        }
    }

    pub(super) fn on_pane_git_ready(&mut self, pane: PaneId, path: VPath, info: Option<GitStatus>) {
        self.pane_mut(pane).git.finish(&path, info);
    }
}

/// Refresh branch/index changes even when they do not alter the listed folder.
pub(super) fn install_refresh(window: &adw::ApplicationWindow, sender: &ComponentSender<AppModel>) {
    let weak = window.downgrade();
    let input = sender.input_sender().clone();
    glib::timeout_add_local(REFRESH_INTERVAL, move || {
        let Some(window) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if window.is_mapped() && input.send(AppMsg::RefreshPaneGit).is_err() {
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    let input = sender.input_sender().clone();
    window.connect_is_active_notify(move |window| {
        if window.is_active() {
            let _ = input.send(AppMsg::RefreshPaneGit);
        }
    });
}

fn read_status(path: &VPath, cancel: &CancelToken) -> Option<GitStatus> {
    let mut command = Command::new("git");
    // Directory context must come from the pane, even if Commander was launched by Git.
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
    ] {
        command.env_remove(variable);
    }
    command
        .arg("--no-optional-locks")
        .args(["-c", "core.fsmonitor=false", "-C"])
        .arg(path.as_path())
        .args([
            "status",
            "--porcelain=v2",
            "--branch",
            "--ahead-behind",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=dirty",
            "--no-renames",
        ]);
    query(command, cancel, QUERY_TIMEOUT)
}

fn query(mut command: Command, cancel: &CancelToken, timeout: Duration) -> Option<GitStatus> {
    if cancel.is_cancelled() {
        return None;
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    // Stream records without retaining file lists or blocking Git on a full output pipe.
    let reader = match thread::Builder::new()
        .name("commander-git-output".to_owned())
        .spawn(move || parse_status(BufReader::new(stdout)))
    {
        Ok(reader) => reader,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let deadline = Instant::now() + timeout;
    let success = loop {
        if cancel.is_cancelled() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let info = reader.join().ok().flatten();
    success.then_some(info).flatten()
}

fn parse_status(reader: impl BufRead) -> Option<GitStatus> {
    let mut info = GitStatus::default();
    let mut skip_source = false;
    for record in reader.split(0) {
        let record = record.ok()?;
        if skip_source {
            skip_source = false;
            continue;
        }
        let text = String::from_utf8_lossy(&record);
        if let Some(value) = text.strip_prefix("# branch.head ") {
            info.branch = value.to_owned();
        } else if let Some(value) = text.strip_prefix("# branch.oid ") {
            info.oid = value.to_owned();
        } else if let Some(value) = text.strip_prefix("# branch.upstream ") {
            info.upstream = Some(value.to_owned());
        } else if let Some(value) = text.strip_prefix("# branch.ab ") {
            let (ahead, behind) = value.split_once(' ')?;
            info.ahead = ahead.strip_prefix('+')?.parse().ok()?;
            info.behind = behind.strip_prefix('-')?.parse().ok()?;
        } else if matches!(record.first(), Some(b'1' | b'2' | b'?' | b'u')) {
            info.changed = true;
            info.conflicts |= record.first() == Some(&b'u');
            skip_source = record.first() == Some(&b'2');
        }
    }
    (!info.branch.is_empty()).then_some(info)
}

pub(super) struct GitStatusWidgets {
    pub(super) root: gtk::Box,
    branch: gtk::Label,
    summary: gtk::Label,
    rendered: Option<GitStatus>,
}

impl GitStatusWidgets {
    pub(super) fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        root.add_css_class("pane-git-status");
        root.set_visible(false);
        root.set_valign(gtk::Align::Center);
        let icon = gtk::Image::from_icon_name("commander-git-branch-symbolic");
        icon.set_pixel_size(14);
        let branch = gtk::Label::new(None);
        branch.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        branch.set_max_width_chars(22);
        branch.set_xalign(0.0);
        let summary = gtk::Label::new(None);
        root.append(&icon);
        root.append(&branch);
        root.append(&summary);
        Self {
            root,
            branch,
            summary,
            rendered: None,
        }
    }

    pub(super) fn render(&mut self, info: Option<&GitStatus>) {
        if self.rendered.as_ref() == info {
            return;
        }
        if let Some(info) = info {
            self.branch.set_label(&info.branch_label());
            let summary = info.summary();
            self.summary.set_label(&summary);
            self.summary.set_visible(!summary.is_empty());
            let tooltip = info.tooltip();
            self.root.set_tooltip_text(Some(&tooltip));
            self.root
                .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
        }
        self.root.set_visible(info.is_some());
        self.rendered = info.cloned();
    }
}

#[cfg(test)]
mod tests;
