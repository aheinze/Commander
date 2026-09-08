use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::commands::{KeymapOverrides, KeymapProfile};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceMode {
    System,
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorTheme {
    #[default]
    #[serde(alias = "carelo")]
    Automatic,
    #[serde(rename = "carelo-graphite")]
    Carelo,
    Midnight,
    Forest,
    Aubergine,
}

/// Persisted presentation mode for a file pane.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneViewMode {
    #[default]
    List,
    Grid,
    Columns,
}

/// Persisted sort field kept independent from the domain crate's serialization.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneSortKey {
    #[default]
    Name,
    Size,
    Modified,
    Type,
}

/// Presentation remembered for a particular folder. Names are restored only if still present.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct FolderViewSession {
    pub view_mode: PaneViewMode,
    pub sort_key: PaneSortKey,
    pub sort_descending: bool,
    pub show_hidden: bool,
    pub column_width: i32,
    pub scroll_y: u32,
    pub cursor_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct NavigationSession {
    pub columns: Vec<String>,
    pub selected_names: Vec<String>,
    pub horizontal_scroll: u32,
}

/// Persisted state for one pane.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PaneSession {
    #[serde(default)]
    pub folders: BTreeMap<String, FolderViewSession>,
    #[serde(default)]
    pub locations: BTreeMap<String, NavigationSession>,
    pub tabs: Vec<String>,
    pub active_tab: usize,
    #[serde(default)]
    pub view_mode: PaneViewMode,
    #[serde(default)]
    pub sort_key: PaneSortKey,
    #[serde(default)]
    pub sort_descending: bool,
    #[serde(default)]
    pub show_hidden: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceSession {
    pub name: String,
    pub left: String,
    pub right: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_pane: Option<PaneSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_pane: Option<PaneSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_pane: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dual_pane: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_split: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_position: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_width: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FavoriteGroupSession {
    pub name: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CustomToolSession {
    pub name: String,
    pub command: String,
    #[serde(default = "default_tool_target")]
    pub applies_to: String,
    #[serde(default)]
    pub extensions: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Global workflow choices, with backwards-compatible defaults for existing sessions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct WorkflowPreferences {
    pub restore_tabs: bool,
    pub browse_archives: bool,
    pub directories_first: bool,
    pub inspector_folder_sizes: bool,
    pub inspector_git: bool,
    pub remember_recent: bool,
    pub recent_limit: u32,
}

impl Default for WorkflowPreferences {
    fn default() -> Self {
        Self {
            restore_tabs: true,
            browse_archives: true,
            directories_first: true,
            inspector_folder_sizes: true,
            inspector_git: true,
            remember_recent: true,
            recent_limit: 12,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionState {
    #[serde(default)]
    pub workflow: WorkflowPreferences,
    #[serde(default)]
    pub vertical_split: bool,
    #[serde(default = "default_dual_pane")]
    pub dual_pane: bool,
    #[serde(default = "default_split_position")]
    pub split_position: i32,
    #[serde(default)]
    pub left: PaneSession,
    #[serde(default)]
    pub right: PaneSession,
    #[serde(default)]
    pub keymap_profile: KeymapProfile,
    #[serde(default = "default_sidebar_visible")]
    pub sidebar_visible: bool,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub collapsed_sidebar_groups: BTreeSet<String>,
    #[serde(default = "default_preview_visible")]
    pub preview_visible: bool,
    #[serde(default = "default_preview_width")]
    pub preview_width: i32,
    #[serde(default)]
    pub bookmarks: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bookmark_labels: BTreeMap<String, String>,
    #[serde(default)]
    pub favorite_groups: Vec<FavoriteGroupSession>,
    #[serde(default)]
    pub recent: Vec<String>,
    #[serde(default = "default_window_width")]
    pub window_width: i32,
    #[serde(default = "default_window_height")]
    pub window_height: i32,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSession>,
    #[serde(default)]
    pub remote_uris: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub remote_names: BTreeMap<String, String>,
    #[serde(default)]
    pub appearance: AppearanceMode,
    #[serde(default)]
    pub color_theme: ColorTheme,
    #[serde(default = "default_parallel_transfers")]
    pub parallel_transfers: bool,
    #[serde(default)]
    pub custom_tools: Vec<CustomToolSession>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            workflow: WorkflowPreferences::default(),
            vertical_split: false,
            dual_pane: true,
            split_position: 600,
            left: PaneSession::default(),
            right: PaneSession::default(),
            keymap_profile: KeymapProfile::default(),
            sidebar_visible: true,
            collapsed_sidebar_groups: BTreeSet::new(),
            preview_visible: true,
            preview_width: default_preview_width(),
            bookmarks: Vec::new(),
            bookmark_labels: BTreeMap::new(),
            favorite_groups: Vec::new(),
            recent: Vec::new(),
            window_width: default_window_width(),
            window_height: default_window_height(),
            workspaces: Vec::new(),
            remote_uris: Vec::new(),
            remote_names: BTreeMap::new(),
            appearance: AppearanceMode::default(),
            color_theme: ColorTheme::default(),
            parallel_transfers: default_parallel_transfers(),
            custom_tools: Vec::new(),
            tags: BTreeMap::new(),
        }
    }
}

const fn default_split_position() -> i32 {
    600
}

const fn default_dual_pane() -> bool {
    true
}

const fn default_sidebar_visible() -> bool {
    true
}

const fn default_preview_visible() -> bool {
    true
}

const fn default_preview_width() -> i32 {
    340
}

const fn default_parallel_transfers() -> bool {
    true
}

fn default_tool_target() -> String {
    "both".to_owned()
}

const fn default_true() -> bool {
    true
}

const fn default_window_width() -> i32 {
    1_520
}

const fn default_window_height() -> i32 {
    900
}

/// Files loaded before GTK starts. Disk access happens on the session worker.
#[derive(Debug, Default)]
pub struct StartupState {
    pub(crate) history: crate::history_store::HistoryState,
    pub(crate) history_warning: Option<String>,
    pub session: Option<SessionState>,
    pub keymap_overrides: KeymapOverrides,
}

/// Session-worker setup failures.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("failed to spawn session worker: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("session worker stopped during startup")]
    StartupDisconnected,
}

/// Dedicated session I/O worker. UI code only sends immutable values through a channel.
pub struct SessionWorker {
    sender: Sender<SessionCommand>,
    worker: Option<JoinHandle<()>>,
}

impl SessionWorker {
    /// Starts the worker and returns any saved state loaded on that thread.
    ///
    /// # Errors
    ///
    /// Returns thread setup or startup-channel failures.
    pub fn start() -> Result<(Self, StartupState), SessionError> {
        let (sender, receiver) = mpsc::channel();
        let (initial_sender, initial_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("dualpane-session".to_owned())
            .spawn(move || session_loop(receiver, initial_sender))
            .map_err(SessionError::Spawn)?;
        let initial = initial_receiver
            .recv()
            .map_err(|_| SessionError::StartupDisconnected)?;
        Ok((
            Self {
                sender,
                worker: Some(worker),
            },
            initial,
        ))
    }

    pub(crate) fn history_writer(&self) -> HistoryWriter {
        HistoryWriter(self.sender.clone())
    }

    /// Queues a complete state snapshot for an atomic worker-thread write.
    pub fn save(&self, state: SessionState) {
        let _ = self.sender.send(SessionCommand::Save(Box::new(state)));
    }

    pub fn save_keymap(
        &self,
        overrides: KeymapOverrides,
        reply: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        if let Err(error) = self
            .sender
            .send(SessionCommand::Keymap(overrides, Box::new(reply)))
            && let SessionCommand::Keymap(_, reply) = error.0
        {
            reply(Err("The settings worker has stopped".to_owned()));
        }
    }
}

impl Drop for SessionWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(SessionCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone)]
pub(crate) struct HistoryWriter(Sender<SessionCommand>);
impl HistoryWriter {
    pub(crate) fn save(&self, state: crate::history_store::HistoryState) {
        let _ = self.0.send(SessionCommand::History(Box::new(state), None));
    }
    pub(crate) fn save_confirmed(
        &self,
        state: crate::history_store::HistoryState,
    ) -> Result<(), String> {
        let (send, receive) = mpsc::sync_channel(1);
        self.0
            .send(SessionCommand::History(Box::new(state), Some(send)))
            .map_err(|_| "History worker stopped".to_owned())?;
        receive
            .recv()
            .map_err(|_| "History worker stopped".to_owned())?
    }
}

enum SessionCommand {
    Keymap(KeymapOverrides, Box<dyn FnOnce(Result<(), String>) + Send>),
    History(
        Box<crate::history_store::HistoryState>,
        Option<mpsc::SyncSender<Result<(), String>>>,
    ),
    Save(Box<SessionState>),
    Shutdown,
}

fn session_loop(
    receiver: Receiver<SessionCommand>,
    initial_sender: mpsc::SyncSender<StartupState>,
) {
    let path = session_path();
    let (history, history_warning) = crate::history_store::path()
        .as_deref()
        .map(crate::history_store::load)
        .unwrap_or_default();
    let initial = StartupState {
        history,
        history_warning,
        session: path.as_deref().and_then(load_session),
        keymap_overrides: keymap_path()
            .as_deref()
            .and_then(load_keymap_overrides)
            .unwrap_or_default(),
    };
    let _ = initial_sender.send(initial);

    while let Ok(command) = receiver.recv() {
        match command {
            SessionCommand::Keymap(overrides, reply) => {
                let result = keymap_path()
                    .ok_or_else(|| "No configuration directory is available".to_owned())
                    .and_then(|path| {
                        let contents = toml_edit::ser::to_string_pretty(&overrides)
                            .map_err(|error| error.to_string())?;
                        crate::history_store::write_atomic(&path, contents.as_bytes())
                            .map_err(|error| error.to_string())
                    });
                reply(result);
            }
            SessionCommand::Save(state) => {
                if let Some(path) = path.as_deref()
                    && let Err(error) = save_session(path, &state)
                {
                    tracing::warn!(%error, "failed to persist session");
                }
            }
            SessionCommand::History(state, acknowledgement) => {
                let result = crate::history_store::path()
                    .ok_or_else(|| "No history state directory is available".to_owned())
                    .and_then(|path| crate::history_store::save(&path, &state));
                if let Err(error) = &result {
                    tracing::error!(%error, "failed to persist undo history");
                }
                if let Some(acknowledgement) = acknowledgement {
                    let _ = acknowledgement.send(result);
                }
            }
            SessionCommand::Shutdown => return,
        }
    }
}

pub(crate) fn state_directory() -> Option<PathBuf> {
    let project = ProjectDirs::from("org", "example", "Dualpane")?;
    Some(
        project
            .state_dir()
            .unwrap_or_else(|| project.data_local_dir())
            .to_owned(),
    )
}

fn session_path() -> Option<PathBuf> {
    let project = ProjectDirs::from("org", "example", "Dualpane")?;
    let state = project
        .state_dir()
        .unwrap_or_else(|| project.data_local_dir());
    Some(state.join("session.toml"))
}

fn keymap_path() -> Option<PathBuf> {
    let project = ProjectDirs::from("org", "example", "Dualpane")?;
    Some(project.config_dir().join("keymaps.toml"))
}

fn load_keymap_overrides(path: &Path) -> Option<KeymapOverrides> {
    let contents = fs::read_to_string(path).ok()?;
    match toml_edit::de::from_str(&contents) {
        Ok(overrides) => Some(overrides),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "ignoring invalid keymap overrides");
            None
        }
    }
}

fn load_session(path: &Path) -> Option<SessionState> {
    let contents = fs::read_to_string(path).ok()?;
    match toml_edit::de::from_str(&contents) {
        Ok(state) => Some(state),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "ignoring invalid session state");
            None
        }
    }
}

fn save_session(path: &Path, state: &SessionState) -> Result<(), Box<dyn std::error::Error>> {
    let contents = toml_edit::ser::to_string_pretty(state)?;
    crate::history_store::write_atomic(path, contents.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{
        AppearanceMode, ColorTheme, CustomToolSession, FavoriteGroupSession, PaneSession,
        SessionState, WorkspaceSession,
    };

    #[test]
    fn session_round_trips_through_toml_edit() {
        let state = SessionState {
            workflow: super::WorkflowPreferences::default(),
            vertical_split: true,
            dual_pane: false,
            split_position: 420,
            left: PaneSession {
                tabs: vec!["/home".to_owned(), "/tmp".to_owned()],
                active_tab: 1,
                view_mode: Default::default(),
                sort_key: Default::default(),
                sort_descending: false,
                show_hidden: false,
                ..PaneSession::default()
            },
            right: PaneSession {
                tabs: vec!["/".to_owned()],
                active_tab: 0,
                view_mode: Default::default(),
                sort_key: Default::default(),
                sort_descending: false,
                show_hidden: false,
                ..PaneSession::default()
            },
            keymap_profile: Default::default(),
            sidebar_visible: true,
            collapsed_sidebar_groups: BTreeSet::from([
                "devices".to_owned(),
                "favorite:Projects".to_owned(),
            ]),
            preview_visible: true,
            preview_width: 412,
            bookmarks: vec!["/home".to_owned()],
            bookmark_labels: BTreeMap::from([("/home".to_owned(), "Personal".to_owned())]),
            favorite_groups: vec![FavoriteGroupSession {
                name: "Projects".to_owned(),
                paths: vec!["/home/project".to_owned()],
                labels: BTreeMap::from([("/home/project".to_owned(), "Work / API".to_owned())]),
            }],
            recent: vec!["/tmp".to_owned()],
            window_width: 1_440,
            window_height: 880,
            workspaces: vec![WorkspaceSession {
                name: "Code".to_owned(),
                left: "/home".to_owned(),
                right: "/tmp".to_owned(),
                left_pane: Some(PaneSession {
                    tabs: vec!["/home".to_owned(), "/home/project".to_owned()],
                    active_tab: 1,
                    ..PaneSession::default()
                }),
                right_pane: Some(PaneSession {
                    tabs: vec!["/tmp".to_owned()],
                    active_tab: 0,
                    ..PaneSession::default()
                }),
                active_pane: Some(1),
                dual_pane: Some(true),
                vertical_split: Some(false),
                split_position: Some(560),
                sidebar_visible: Some(true),
                preview_visible: Some(false),
                preview_width: Some(340),
            }],
            remote_uris: vec!["sftp://example.com".to_owned()],
            remote_names: BTreeMap::from([(
                "sftp://example.com".to_owned(),
                "Work server".to_owned(),
            )]),
            appearance: AppearanceMode::System,
            color_theme: ColorTheme::Forest,
            parallel_transfers: false,
            custom_tools: vec![CustomToolSession {
                name: "Inspect".to_owned(),
                command: "file %path%".to_owned(),
                applies_to: "files".to_owned(),
                extensions: String::new(),
                enabled: true,
            }],
            tags: BTreeMap::from([("/tmp".to_owned(), "blue".to_owned())]),
        };

        let encoded = toml_edit::ser::to_string_pretty(&state).expect("encode");
        let decoded: SessionState = toml_edit::de::from_str(&encoded).expect("decode");

        assert_eq!(decoded, state);
    }

    #[test]
    fn favorites_without_custom_labels_remain_loadable() {
        let decoded: SessionState = toml_edit::de::from_str(
            r#"
                bookmarks = ["/home/project"]
                [[favorite_groups]]
                name = "Work"
                paths = ["/home/project"]
            "#,
        )
        .unwrap();
        assert_eq!(decoded.bookmarks, ["/home/project"]);
        assert!(decoded.bookmark_labels.is_empty());
        assert_eq!(decoded.favorite_groups[0].paths, ["/home/project"]);
        assert!(decoded.favorite_groups[0].labels.is_empty());
    }

    #[test]
    fn m2_session_uses_defaults_for_m3_fields() {
        let decoded: SessionState = toml_edit::de::from_str(
            r#"
                vertical_split = false
                split_position = 600
                remote_uris = ["sftp://example.com"]
                [left]
                tabs = ["/tmp"]
                active_tab = 0
                [right]
                tabs = ["/"]
                active_tab = 0
            "#,
        )
        .expect("legacy session");

        assert!(decoded.sidebar_visible);
        assert!(decoded.collapsed_sidebar_groups.is_empty());
        assert!(decoded.dual_pane);
        assert!(decoded.preview_visible);
        assert_eq!(decoded.preview_width, 340);
        assert!(decoded.bookmarks.is_empty());
        assert!(decoded.recent.is_empty());
        assert_eq!(decoded.window_width, 1_520);
        assert_eq!(decoded.window_height, 900);
        assert!(decoded.workspaces.is_empty());
        assert_eq!(decoded.remote_uris, ["sftp://example.com"]);
        assert!(decoded.remote_names.is_empty());
        assert_eq!(decoded.appearance, AppearanceMode::Dark);
        assert!(decoded.tags.is_empty());
        assert_eq!(decoded.keymap_profile, Default::default());
        assert_eq!(decoded.left.sort_key, Default::default());
        assert!(!decoded.left.sort_descending);
        assert!(!decoded.left.show_hidden);
    }

    #[test]
    fn legacy_workspaces_remain_loadable_as_location_pairs() {
        let decoded: SessionState = toml_edit::de::from_str(
            r#"
                [[workspaces]]
                name = "Legacy"
                left = "/home"
                right = "/tmp"
            "#,
        )
        .expect("legacy workspace session");

        let workspace = &decoded.workspaces[0];
        assert_eq!(workspace.name, "Legacy");
        assert_eq!(workspace.left, "/home");
        assert_eq!(workspace.right, "/tmp");
        assert!(workspace.left_pane.is_none());
        assert!(workspace.right_pane.is_none());
        assert!(workspace.dual_pane.is_none());
    }

    #[test]
    fn legacy_carelo_theme_migrates_to_automatic() {
        let decoded: SessionState = toml_edit::de::from_str(
            r#"
                color_theme = "carelo"
                [left]
                tabs = []
                active_tab = 0
                [right]
                tabs = []
                active_tab = 0
            "#,
        )
        .expect("legacy Carelo theme");

        assert_eq!(decoded.color_theme, ColorTheme::Automatic);
        let encoded = toml_edit::ser::to_string_pretty(&decoded).expect("encode");
        assert!(encoded.contains("color_theme = \"automatic\""));
    }
}
