//! The Relm4 application: shared state types plus the module wiring for the UI.
//!
//! `AppModel`, `AppMsg`, `AppWidgets`, and the per-pane state they own live here; everything
//! that acts on them lives in one of the submodules below. `component` holds the
//! `SimpleComponent` implementation, the `model_*` modules hold topical `impl AppModel`
//! blocks, and the remaining modules build one widget surface each.
//!
//! Submodules are files of this one logical module rather than independent units: each opens
//! with `use super::*` to inherit these imports, and exports back to the parent with
//! `pub(super)`.

mod action_policy;
mod alert;
mod archive_actions;
mod archive_browser;
mod archive_password;
mod batch_rename_view;
#[cfg(test)]
mod breadcrumb_tests;
mod chrome;
mod clipboard;
mod compare_view;
mod component;
mod context_menu;
mod devices;
mod dialogs;
#[cfg(test)]
mod favorites_tests;
mod fileops;
mod job_view;
mod layout;
#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod miller_tests;
mod model_archives;
mod model_commands;
mod model_cursor;
mod model_filter;
mod model_inspector;
mod model_jobs;
mod model_metrics;
mod model_miller;
mod model_navigation;
mod model_selection_size;
mod model_tools;
mod model_view;
mod model_watch;
mod navigation_state;
mod notifications;
mod palette;
mod pane_git;
mod pane_view;
mod power_tools;
mod preview;
mod recovery;
mod remote;
mod search_actions;
mod search_view;
mod secure_delete;
mod settings;
mod shortcuts;
mod sidebar;
mod terminal_view;
mod theme;
mod topbar;
mod util;
mod widgets;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Sender as ChannelSender, SyncSender, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dualpane_core::{
    CancelToken, EntryKind, Filter, Selection, SelectionKey, SizeHint, SortDirection, SortKey,
    SortSpec, VPath,
};
use dualpane_engine::{
    Conflict, ConflictAction, ConflictDecision, ConflictId, ConflictPolicy, JobControl, JobEvent,
    JobHandle, JobId, JobKind, JobPhase, JobProgress, JobState, OperationEngine, ScanOptions,
    TransferOptions, TransferRecord, TrashRecord, unique_renamed_path,
};
use dualpane_index::{
    FilterResult, FuzzyFilter, IndexError, Listing, ListingEvent, ListingRequest, ListingTask,
    MetadataResult, hydrate_metadata_rows,
};
use dualpane_thumbs::{
    Preview, PreviewError, PreviewPayload, Thumbnail, ThumbnailError, ThumbnailFingerprint,
    ThumbnailRequest, ThumbnailResponse, ThumbnailScheduler, load_preview, supports_thumbnail,
};
use dualpane_vfs::{LocalFs, Vfs};
use relm4::adw;
use relm4::adw::prelude::*;
use relm4::gtk;
use relm4::gtk::gdk;
use relm4::gtk::gio;
use relm4::gtk::glib;
use relm4::{ComponentParts, ComponentSender, SimpleComponent};

use crate::archive::{
    ArchiveFormat, ArchiveProgress, ArchiveTask, create_archive, extract_archive,
};
use crate::cli::AppOptions;
use crate::commands::{
    COMMANDS, CommandDefinition, CommandId, Keymap, KeymapOverrides, KeymapProfile,
};
use crate::features::{
    ImageOutputFormat, MAX_CONTENT_BYTES, SearchHit, SearchOptions, SearchResults, convert_image,
    recursive_search, sha256, supports_image_conversion,
};
use crate::list_model::{ListingListModel, RowReference};
use crate::omarchy::{self, OmarchyPalette};
use crate::pdf::{PdfTool, PdfToolOptions, run_pdf_tool};
use crate::session::{
    AppearanceMode, ColorTheme, CustomToolSession, FavoriteGroupSession, FolderViewSession,
    NavigationSession, PaneSession, PaneSortKey, PaneViewMode, SessionState, SessionWorker,
    WorkflowPreferences, WorkspaceSession,
};
use crate::terminal::{TerminalEvent, TerminalSession};

use self::alert::AlertSheet;
use self::context_menu::{
    column_view_toggle_button, context_menu_item_button, icon_button, install_file_context_menu,
    pane_count_toggle_button, view_toggle_button,
};
use self::dialogs::{
    connect_remote_uri, reveal_in_file_manager, show_batch_rename_dialog, show_checksum_comparison,
    show_checksum_result, show_conflict_dialog, show_create_archive_dialog,
    show_delete_workspace_dialog, show_elevated_permissions_dialog, show_image_conversion_dialog,
    show_new_directory_dialog, show_new_favorite_group_dialog, show_new_file_dialog,
    show_open_with_dialog, show_pdf_tools_dialog, show_permanent_delete_dialog,
    show_permissions_dialog, show_rename_dialog, show_rename_workspace_dialog,
    show_save_workspace_dialog, show_shortcut_reference, show_update_workspace_dialog,
};
use self::fileops::{apply_history, batch_rename, common_parent, set_mode_tree};
use self::palette::{matching_commands, palette_query_matches};
use self::pane_view::{PaneWidgets, ScrollMetrics, install_file_drop_target};
use self::preview::{PreviewWidgets, QuickLookWidgets, git_info_for_path, measure_folder_paths};
use self::remote::{forget_remote_password, show_remote_dialog, show_remote_error};
use self::search_view::SearchWidgets;
use self::shortcuts::{
    connect_button, install_shortcuts, palette_delete_backward, palette_insert_character,
};
use self::sidebar::{SidebarWidgets, build_sidebar, sidebar_button, window_controls};
use self::terminal_view::{
    TerminalScreenState, TerminalWidgets, replacement_terminal_index, terminal_title,
};
use self::theme::{
    apply_appearance, apply_color_theme, install_omarchy_theme_monitor, install_styles,
};

use self::util::{
    display_name, estimated_grid_columns, format_mode, format_size, format_timestamp, home_path,
    is_archive_path, job_kind_label, launch_custom_tool, local_trash_path, miller_scroll_target,
    operation_log_label, overridden_session, permission_triplet, plural, preview_type_label,
    relative_log_time, replace_text, split_extension, title_case, valid_file_name,
    wildcard_matches,
};

const METADATA_LOOKBEHIND: u32 = 64;
const METADATA_LOOKAHEAD: u32 = 256;
const SCROLL_BENCHMARK_STEPS: u32 = 120;
const SCROLL_ROWS_PER_STEP: u32 = 1;
const PRESENTATION_ACK_TIMEOUT: Duration = Duration::from_millis(250);
const GRID_THUMBNAIL_EDGE: u32 = 192;
const THUMBNAIL_CACHE_BYTES: usize = 32 * 1_024 * 1_024;
const SIDEBAR_WIDTH: i32 = 250;
const SIDEBAR_LABEL_WIDTH_CHARS: i32 = 16;
const PREVIEW_DEFAULT_WIDTH: i32 = 340;
const PREVIEW_MIN_WIDTH: i32 = 330;
const PREVIEW_MAX_WIDTH: i32 = 560;
const PREVIEW_SETTLE_DELAY: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaneId {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileDropAction {
    Copy,
    Move,
}

#[derive(Default)]
struct FileDragUiState {
    internal_active: Cell<bool>,
    history_busy: Cell<bool>,
    archive_roots: RefCell<Vec<(std::path::PathBuf, bool)>>,
}

struct PaneDragState {
    current_directory: Option<VPath>,
    archive_access: Option<bool>,
    actions: action_policy::Context,
    selected: Vec<VPath>,
    bound: HashMap<usize, (VPath, EntryKind)>,
}

type FileDropDestination = dyn Fn(&gtk::Widget) -> Option<VPath>;

impl PaneDragState {
    fn new() -> Self {
        Self {
            current_directory: None,
            archive_access: None,
            actions: action_policy::Context::default(),
            selected: Vec::new(),
            bound: HashMap::new(),
        }
    }

    fn bind(&mut self, widget: &impl IsA<gtk::Widget>, path: VPath, kind: EntryKind) {
        self.bound
            .insert(widget.as_ref().as_ptr() as usize, (path, kind));
    }

    fn unbind(&mut self, widget: &impl IsA<gtk::Widget>) {
        self.bound.remove(&(widget.as_ref().as_ptr() as usize));
    }

    fn drag_paths(&self, widget: &gtk::Widget) -> Vec<VPath> {
        let Some((dragged, _)) = self.bound.get(&(widget.as_ptr() as usize)) else {
            return Vec::new();
        };
        if self.selected.contains(dragged) {
            self.selected.clone()
        } else {
            vec![dragged.clone()]
        }
    }

    fn folder_for_widget(&self, widget: &gtk::Widget) -> Option<VPath> {
        self.bound
            .get(&(widget.as_ptr() as usize))
            .filter(|(_, kind)| kind.is_directory())
            .map(|(path, _)| path.clone())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum InspectorPage {
    #[default]
    Info,
    Work,
    Log,
}

impl InspectorPage {
    const fn name(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Work => "work",
            Self::Log => "log",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Info => "Information",
            Self::Work => "Current Work",
            Self::Log => "Activity Log",
        }
    }
}

impl PaneViewMode {
    const fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Grid => "grid",
            Self::Columns => "columns",
        }
    }
}

struct OperationStatus {
    phase: JobPhase,
    kind: OperationKind,
    state: JobState,
    progress: JobProgress,
    control: JobControl,
    commands: ChannelSender<JobBridgeCommand>,
    retry: OperationRetry,
    waiting_for_conflict: bool,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationKind {
    Files(JobKind),
    SecureDelete,
    CreateArchive,
    ExtractArchive,
    UpdateArchive,
}

impl OperationKind {
    fn label(self) -> &'static str {
        match self {
            Self::Files(kind) => job_kind_label(kind),
            Self::SecureDelete => "Secure delete",
            Self::CreateArchive => "Create archive",
            Self::ExtractArchive => "Extract archive",
            Self::UpdateArchive => "Update archive",
        }
    }

    fn is_archive(self) -> bool {
        matches!(
            self,
            Self::CreateArchive | Self::ExtractArchive | Self::UpdateArchive
        )
    }
}

#[derive(Clone)]
enum OperationRetry {
    UpdateArchive {
        pane: PaneId,
        request: archive_actions::Request,
    },
    SecureDelete {
        sources: Vec<VPath>,
    },
    Copy {
        pane: PaneId,
        sources: Vec<VPath>,
        destination: VPath,
    },
    Move {
        pane: PaneId,
        sources: Vec<VPath>,
        destination: VPath,
    },
    Trash {
        pane: PaneId,
        sources: Vec<VPath>,
    },
    Delete {
        pane: PaneId,
        sources: Vec<VPath>,
    },
    Archive {
        pane: PaneId,
        sources: Vec<VPath>,
        destination: VPath,
        format: Option<ArchiveFormat>,
        password: Option<crate::archive::Password>,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ConflictChoice {
    Replace,
    Skip,
    KeepBoth,
    ReplaceIfNewer,
}

#[derive(Debug)]
enum JobBridgeCommand {
    Resolve {
        conflict_id: ConflictId,
        choice: ConflictChoice,
        apply_to_all: bool,
    },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum HistoryEntry {
    Archive {
        change: dualpane_engine::journal::ArchiveChange,
    },
    Replaced {
        kind: JobKind,
        records: Vec<TransferRecord>,
        backups: Vec<dualpane_engine::journal::BackupRecord>,
        trashed: Vec<TrashRecord>,
    },
    Copy {
        records: Vec<TransferRecord>,
        trashed: Vec<TrashRecord>,
    },
    Move {
        records: Vec<TransferRecord>,
    },
    Rename {
        from: VPath,
        to: VPath,
    },
    BatchRename {
        moves: Vec<(VPath, VPath)>,
    },
    Trash {
        records: Vec<TrashRecord>,
    },
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum HistoryDirection {
    Undo,
    Redo,
}

struct PreviewState {
    generation: u64,
    path: Option<VPath>,
    cancel: Option<CancelToken>,
    content: Option<Preview>,
    error: Option<String>,
    loading: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SelectionSummary {
    count: usize,
    files: usize,
    folders: usize,
    other: usize,
    known_bytes: u64,
    unknown_sizes: usize,
    folder_paths: Vec<VPath>,
    location: Option<VPath>,
}

struct InspectorSelectionState {
    generation: u64,
    loading: bool,
    summary: Option<SelectionSummary>,
}

impl InspectorSelectionState {
    const fn new() -> Self {
        Self {
            generation: 0,
            loading: false,
            summary: None,
        }
    }

    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.loading = false;
        self.summary = None;
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FolderMeasureResult {
    bytes: u64,
    items: u64,
    skipped: u64,
}

struct FolderMeasureState {
    generation: u64,
    loading: bool,
    result: Option<FolderMeasureResult>,
    error: Option<String>,
    cancel: Option<CancelToken>,
}

impl FolderMeasureState {
    fn new() -> Self {
        Self {
            generation: 0,
            loading: false,
            result: None,
            error: None,
            cancel: None,
        }
    }

    fn reset(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
        self.loading = false;
        self.result = None;
        self.error = None;
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GitInfo {
    /// Branch name, or a short description of a detached HEAD.
    branch: String,
    /// Tracking branch, when one is configured.
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    /// State of the inspected item itself.
    item_state: GitItemState,
    /// Human summary of the inspected item ("Modified", "3 modified, 1 untracked").
    item_summary: String,
    /// Human summary of the whole working tree.
    tree_summary: String,
    /// Most recent commit touching the inspected item.
    last_commit: Option<GitCommit>,
    root: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitItemState {
    Clean,
    Modified,
    Staged,
    Untracked,
    Ignored,
    Conflict,
}

#[derive(Clone, Debug)]
pub(crate) struct GitCommit {
    hash: String,
    subject: String,
    relative_time: String,
    author: String,
}

struct InspectorGitState {
    generation: u64,
    loading: bool,
    info: Option<GitInfo>,
}

impl InspectorGitState {
    const fn new() -> Self {
        Self {
            generation: 0,
            loading: false,
            info: None,
        }
    }

    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.loading = false;
        self.info = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationLogStatus {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
struct OperationLogEntry {
    id: u64,
    created_at: SystemTime,
    status: OperationLogStatus,
    label: String,
    detail: String,
}

impl PreviewState {
    fn new() -> Self {
        Self {
            generation: 0,
            path: None,
            cancel: None,
            content: None,
            error: None,
            loading: false,
        }
    }

    fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.loading = false;
    }
}

struct ThumbnailUiEvent {
    path: VPath,
    thumbnail: Option<Thumbnail>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct GridThumbnailKey {
    pane: usize,
    generation: u64,
    path: VPath,
}

struct CachedThumbnail {
    texture: gdk::MemoryTexture,
    fingerprint: ThumbnailFingerprint,
    bytes: usize,
    last_used: u64,
}

struct ThumbnailTarget {
    widget_id: usize,
    key: GridThumbnailKey,
    stack: glib::WeakRef<gtk::Stack>,
    picture: glib::WeakRef<gtk::Picture>,
}

struct ThumbnailUiState {
    active_generations: [u64; 2],
    requested: HashSet<GridThumbnailKey>,
    current: HashMap<usize, GridThumbnailKey>,
    waiting: HashMap<VPath, Vec<ThumbnailTarget>>,
    cache: HashMap<VPath, CachedThumbnail>,
    cache_bytes: usize,
    clock: u64,
}

impl ThumbnailUiState {
    fn new() -> Self {
        Self {
            active_generations: [u64::MAX; 2],
            requested: HashSet::new(),
            current: HashMap::new(),
            waiting: HashMap::new(),
            cache: HashMap::new(),
            cache_bytes: 0,
            clock: 0,
        }
    }

    fn begin_generation(&mut self, pane: PaneId, generation: u64) -> HashSet<VPath> {
        let pane = pane.index();
        if self.active_generations[pane] == generation {
            return HashSet::new();
        }
        let invalidated = self
            .waiting
            .iter()
            .filter(|(_, targets)| targets.iter().any(|target| target.key.pane == pane))
            .map(|(path, _)| path.clone())
            .collect();
        self.active_generations[pane] = generation;
        self.requested
            .retain(|key| key.pane != pane || key.generation == generation);
        self.current
            .retain(|_, key| key.pane != pane || key.generation == generation);
        self.waiting.retain(|_, targets| {
            targets.retain(|target| target.key.pane != pane || target.key.generation == generation);
            !targets.is_empty()
        });
        invalidated
    }

    fn bind(
        &mut self,
        pane: PaneId,
        generation: u64,
        path: &VPath,
        fingerprint: Option<ThumbnailFingerprint>,
        stack: &gtk::Stack,
        picture: &gtk::Picture,
    ) -> bool {
        let widget_id = stack.as_ptr() as usize;
        let key = GridThumbnailKey {
            pane: pane.index(),
            generation,
            path: path.clone(),
        };
        self.current.insert(widget_id, key.clone());
        picture.set_paintable(Option::<&gdk::Paintable>::None);
        stack.set_visible_child_name("icon");

        if let Some(fingerprint) = fingerprint
            && self
                .cache
                .get(path)
                .is_some_and(|cached| cached.fingerprint != fingerprint)
        {
            self.remove_cached(path);
        }

        let exact_cache_hit = fingerprint.is_some_and(|fingerprint| {
            self.cache
                .get(path)
                .is_some_and(|cached| cached.fingerprint == fingerprint)
        });
        if let Some(texture) = self.cached_texture(path) {
            picture.set_paintable(Some(&texture));
            stack.set_visible_child_name("thumbnail");
        }
        if exact_cache_hit {
            return false;
        }

        let targets = self.waiting.entry(path.clone()).or_default();
        targets.retain(|target| target.stack.upgrade().is_some());
        targets.retain(|target| target.widget_id != widget_id);
        targets.push(ThumbnailTarget {
            widget_id,
            key: key.clone(),
            stack: stack.downgrade(),
            picture: picture.downgrade(),
        });
        self.requested.insert(key)
    }

    fn unbind(&mut self, stack: &gtk::Stack, picture: &gtk::Picture) {
        self.current.remove(&(stack.as_ptr() as usize));
        picture.set_paintable(Option::<&gdk::Paintable>::None);
        stack.set_visible_child_name("icon");
    }

    fn complete(&mut self, path: &VPath, thumbnail: Option<&Thumbnail>) {
        let texture = if let Some(thumbnail) = thumbnail {
            let texture = thumbnail_texture(thumbnail);
            if let Some(texture) = &texture {
                self.insert_cached(path.clone(), texture.clone(), thumbnail);
            }
            texture
        } else {
            self.cached_texture(path)
        };

        let Some(targets) = self.waiting.remove(path) else {
            return;
        };
        for target in targets {
            if self.current.get(&target.widget_id) != Some(&target.key)
                || self.active_generations[target.key.pane] != target.key.generation
            {
                continue;
            }
            let (Some(stack), Some(picture)) = (target.stack.upgrade(), target.picture.upgrade())
            else {
                self.current.remove(&target.widget_id);
                continue;
            };
            if let Some(texture) = &texture {
                picture.set_paintable(Some(texture));
                stack.set_visible_child_name("thumbnail");
            } else {
                picture.set_paintable(Option::<&gdk::Paintable>::None);
                stack.set_visible_child_name("icon");
            }
        }
    }

    fn cached_texture(&mut self, path: &VPath) -> Option<gdk::MemoryTexture> {
        self.clock = self.clock.wrapping_add(1);
        let cached = self.cache.get_mut(path)?;
        cached.last_used = self.clock;
        Some(cached.texture.clone())
    }

    fn insert_cached(&mut self, path: VPath, texture: gdk::MemoryTexture, thumbnail: &Thumbnail) {
        self.remove_cached(&path);
        self.clock = self.clock.wrapping_add(1);
        let bytes = thumbnail.rgba.len();
        self.cache_bytes = self.cache_bytes.saturating_add(bytes);
        self.cache.insert(
            path,
            CachedThumbnail {
                texture,
                fingerprint: thumbnail.fingerprint,
                bytes,
                last_used: self.clock,
            },
        );
        while self.cache_bytes > THUMBNAIL_CACHE_BYTES && self.cache.len() > 1 {
            let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            self.remove_cached(&oldest);
        }
    }

    fn remove_cached(&mut self, path: &VPath) {
        if let Some(cached) = self.cache.remove(path) {
            self.cache_bytes = self.cache_bytes.saturating_sub(cached.bytes);
        }
    }
}

fn thumbnail_texture(thumbnail: &Thumbnail) -> Option<gdk::MemoryTexture> {
    let width = i32::try_from(thumbnail.width).ok()?;
    let height = i32::try_from(thumbnail.height).ok()?;
    let stride = usize::try_from(thumbnail.width).ok()?.checked_mul(4)?;
    let bytes = glib::Bytes::from_owned(Arc::clone(&thumbnail.rgba));
    Some(gdk::MemoryTexture::new(
        width,
        height,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    ))
}

impl PaneId {
    const fn other(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }
}

#[derive(Debug)]
struct TabState {
    path: VPath,
    history: Vec<VPath>,
    history_index: usize,
    base_listing: Option<Arc<Listing>>,
    listing: Option<Arc<Listing>>,
}

impl TabState {
    fn new(path: VPath) -> Self {
        Self {
            history: vec![path.clone()],
            path,
            history_index: 0,
            base_listing: None,
            listing: None,
        }
    }

    fn navigate(&mut self, path: VPath) {
        self.history.truncate(self.history_index + 1);
        self.history.push(path.clone());
        self.history_index = self.history.len() - 1;
        self.path = path;
        self.base_listing = None;
        self.listing = None;
    }

    fn back(&mut self) -> bool {
        if self.history_index == 0 {
            return false;
        }
        self.history_index -= 1;
        self.path = self.history[self.history_index].clone();
        self.base_listing = None;
        self.listing = None;
        true
    }

    fn forward(&mut self) -> bool {
        if self.history_index + 1 >= self.history.len() {
            return false;
        }
        self.history_index += 1;
        self.path = self.history[self.history_index].clone();
        self.base_listing = None;
        self.listing = None;
        true
    }
}

enum PaneFilterCommand {
    Query {
        generation: u64,
        query: String,
        queued_at: Instant,
    },
}

#[derive(Debug)]
struct MillerColumnState {
    path: VPath,
    base_listing: Option<Arc<Listing>>,
    restore_name: Option<OsString>,
    scroll_y: u32,
    listing: Option<Arc<Listing>>,
    selected_row: Option<u32>,
    loading: bool,
    error: Option<String>,
    width: i32,
}

struct PaneState {
    archive_browse: archive_browser::ArchiveBrowseState,
    git: pane_git::PaneGitState,
    folder_views: BTreeMap<String, FolderViewSession>,
    locations: BTreeMap<String, NavigationSession>,
    restore_names: Vec<String>,
    restore_cursor: Option<String>,
    pending_reveal: Option<VPath>,
    reveal_epoch: u64,
    scroll_y: u32,
    miller_scroll_x: u32,
    scroll_restore_epoch: u64,
    tabs: Vec<TabState>,
    active_tab: usize,
    view_mode: PaneViewMode,
    sort: SortSpec,
    show_hidden: bool,
    generation: u64,
    revision: u64,
    metadata_revision: u64,
    selection_revision: u64,
    selection_size: FolderMeasureState,
    selection_size_stamp: Option<[u64; 4]>,
    tabs_revision: u64,
    loading: bool,
    error: Option<String>,
    listing_cancel: Option<CancelToken>,
    thumbnail_cancel: CancelToken,
    presentation_ack: Option<SyncSender<()>>,
    metadata_cancel: Option<CancelToken>,
    metadata_inflight: bool,
    metadata_pending: BTreeSet<u32>,
    filter_cancel: Option<CancelToken>,
    filter_sender: Option<ChannelSender<PaneFilterCommand>>,
    filter_generation: u64,
    filter_ready: bool,
    filtering: bool,
    filter_query: String,
    filter_elapsed: Option<Duration>,
    selection: Selection,
    cursor_row: u32,
    grid_columns: u32,
    range_anchor: Option<u32>,
    focus_filter_epoch: u64,
    focus_files_epoch: u64,
    typeahead_query: String,
    typeahead_updated: Option<Instant>,
    glob_cancel: Option<CancelToken>,
    glob_generation: u64,
    glob_open: bool,
    glob_select: bool,
    glob_query: String,
    miller_columns: Vec<MillerColumnState>,
    miller_focus: Option<(VPath, EntryKind)>,
    miller_generation: u64,
    miller_revision: u64,
    miller_cancel: Option<CancelToken>,
    workers: Vec<JoinHandle<()>>,
}

impl PaneState {
    fn from_session(session: &PaneSession, fallback: VPath) -> Self {
        let tabs: Vec<_> = session
            .tabs
            .iter()
            .filter(|path| !path.is_empty())
            .map(|path| TabState::new(VPath::from(path.as_str())))
            .collect();
        let tabs = if tabs.is_empty() {
            vec![TabState::new(fallback)]
        } else {
            tabs
        };
        let active_tab = session.active_tab.min(tabs.len() - 1);
        let mut state = Self {
            archive_browse: archive_browser::ArchiveBrowseState::default(),
            git: pane_git::PaneGitState::default(),
            folder_views: session.folders.clone(),
            locations: session.locations.clone(),
            restore_names: Vec::new(),
            restore_cursor: None,
            pending_reveal: None,
            reveal_epoch: 0,
            scroll_y: 0,
            miller_scroll_x: 0,
            scroll_restore_epoch: 0,
            tabs,
            active_tab,
            view_mode: session.view_mode,
            sort: SortSpec {
                key: match session.sort_key {
                    PaneSortKey::Name => SortKey::Name,
                    PaneSortKey::Size => SortKey::Size,
                    PaneSortKey::Modified => SortKey::Modified,
                    PaneSortKey::Type => SortKey::Kind,
                },
                direction: if session.sort_descending {
                    SortDirection::Descending
                } else {
                    SortDirection::Ascending
                },
                directories_first: true,
            },
            show_hidden: session.show_hidden,
            generation: 0,
            revision: 0,
            metadata_revision: 0,
            selection_revision: 0,
            selection_size: FolderMeasureState::new(),
            selection_size_stamp: None,
            tabs_revision: 0,
            loading: false,
            error: None,
            listing_cancel: None,
            thumbnail_cancel: CancelToken::new(),
            presentation_ack: None,
            metadata_cancel: None,
            metadata_inflight: false,
            metadata_pending: BTreeSet::new(),
            filter_cancel: None,
            filter_sender: None,
            filter_generation: 0,
            filter_ready: false,
            filtering: false,
            filter_query: String::new(),
            filter_elapsed: None,
            selection: Selection::new(),
            cursor_row: 0,
            grid_columns: 2,
            range_anchor: None,
            focus_filter_epoch: 0,
            focus_files_epoch: 0,
            typeahead_query: String::new(),
            typeahead_updated: None,
            glob_cancel: None,
            glob_generation: 0,
            glob_open: false,
            glob_select: true,
            glob_query: String::new(),
            miller_columns: Vec::new(),
            miller_focus: None,
            miller_generation: 0,
            miller_revision: 0,
            miller_cancel: None,
            workers: Vec::new(),
        };
        state.restore_navigation();
        state
    }

    fn active(&self) -> &TabState {
        &self.tabs[self.active_tab]
    }

    /// Folder currently being browsed, including an empty or loading Miller column.
    fn current_directory(&self) -> &VPath {
        if self.view_mode == PaneViewMode::Columns
            && let Some(column) = self.miller_columns.last()
        {
            &column.path
        } else {
            &self.active().path
        }
    }

    fn active_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active_tab]
    }

    fn cancel_work(&mut self) {
        self.archive_browse.cancel();
        self.selection_size.reset();
        self.selection_size_stamp = None;
        if let Some(cancel) = self.listing_cancel.take() {
            cancel.cancel();
        }
        self.thumbnail_cancel.cancel();
        self.presentation_ack = None;
        if let Some(cancel) = self.metadata_cancel.take() {
            cancel.cancel();
        }
        if let Some(cancel) = self.filter_cancel.take() {
            cancel.cancel();
        }
        self.filter_sender = None;
        self.filter_ready = false;
        if let Some(cancel) = self.glob_cancel.take() {
            cancel.cancel();
        }
        if let Some(cancel) = self.miller_cancel.take() {
            cancel.cancel();
        }
        self.metadata_inflight = false;
        self.metadata_pending.clear();
        self.filtering = false;
    }

    fn reap_workers(&mut self) {
        let mut active = Vec::new();
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                active.push(worker);
            }
        }
        self.workers = active;
    }

    fn reset_directory_view(&mut self) {
        self.pending_reveal = None;
        self.filter_generation = self.filter_generation.wrapping_add(1);
        self.miller_generation = self.miller_generation.wrapping_add(1);
        self.filter_query.clear();
        self.filter_elapsed = None;
        self.selection.clear();
        self.selection_revision = self.selection_revision.wrapping_add(1);
        self.cursor_row = 0;
        self.range_anchor = None;
        self.typeahead_query.clear();
        self.typeahead_updated = None;
        self.glob_open = false;
        self.glob_query.clear();
        self.miller_columns.clear();
        self.miller_focus = None;
        self.miller_revision = self.miller_revision.wrapping_add(1);
        self.restore_navigation();
    }

    fn to_session(&self) -> PaneSession {
        let (folders, locations) = self.navigation_snapshot();
        PaneSession {
            folders,
            locations,
            tabs: self.tabs.iter().map(|tab| tab.path.to_string()).collect(),
            active_tab: self.active_tab,
            view_mode: self.view_mode,
            sort_key: match self.sort.key {
                SortKey::Name => PaneSortKey::Name,
                SortKey::Size => PaneSortKey::Size,
                SortKey::Modified => PaneSortKey::Modified,
                SortKey::Kind => PaneSortKey::Type,
            },
            sort_descending: self.sort.direction == SortDirection::Descending,
            show_hidden: self.show_hidden,
        }
    }

    fn apply_session(&mut self, session: &PaneSession, fallback: VPath) {
        self.cancel_work();
        self.folder_views = session.folders.clone();
        self.locations = session.locations.clone();
        let tabs = session
            .tabs
            .iter()
            .filter(|path| !path.is_empty())
            .map(|path| TabState::new(VPath::from(path.as_str())))
            .collect::<Vec<_>>();
        self.tabs = if tabs.is_empty() {
            vec![TabState::new(fallback)]
        } else {
            tabs
        };
        self.active_tab = session.active_tab.min(self.tabs.len() - 1);
        self.view_mode = session.view_mode;
        self.sort = SortSpec {
            key: match session.sort_key {
                PaneSortKey::Name => SortKey::Name,
                PaneSortKey::Size => SortKey::Size,
                PaneSortKey::Modified => SortKey::Modified,
                PaneSortKey::Type => SortKey::Kind,
            },
            direction: if session.sort_descending {
                SortDirection::Descending
            } else {
                SortDirection::Ascending
            },
            directories_first: self.sort.directories_first,
        };
        self.show_hidden = session.show_hidden;
        self.error = None;
        self.loading = false;
        self.tabs_revision = self.tabs_revision.wrapping_add(1);
        self.reset_directory_view();
    }
}

impl Drop for PaneState {
    fn drop(&mut self) {
        self.cancel_work();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// Data supplied before GTK takes ownership of the main thread.
pub struct AppInit {
    pub options: AppOptions,
    pub session_worker: SessionWorker,
    pub session: Option<SessionState>,
    pub keymap_overrides: KeymapOverrides,
    pub(crate) history: crate::history_store::HistoryState,
    pub(crate) history_warning: Option<String>,
    pub started: Instant,
}

struct TerminalTabState {
    id: u64,
    title: String,
    cwd: String,
    starting: bool,
    exited: bool,
    session: Option<TerminalSession>,
    screen: Rc<RefCell<TerminalScreenState>>,
}

pub struct AppModel {
    live_updates: model_watch::LiveUpdates,
    devices: devices::DeviceState,
    panes: [PaneState; 2],
    active_pane: PaneId,
    folder_action_target: Option<context_menu::TabFolderTarget>,
    vertical_split: bool,
    dual_pane: bool,
    split_position: i32,
    keymap: Keymap,
    sidebar_visible: bool,
    collapsed_sidebar_groups: BTreeSet<String>,
    preview_visible: bool,
    preview_width: i32,
    quick_look_open: bool,
    inspector_page: InspectorPage,
    preview_state: PreviewState,
    inspector_selection: InspectorSelectionState,
    folder_measure: FolderMeasureState,
    inspector_git: InspectorGitState,
    bookmarks: Vec<VPath>,
    bookmark_labels: BTreeMap<String, String>,
    favorite_groups: Vec<FavoriteGroupSession>,
    recent: Vec<VPath>,
    window_width: i32,
    window_height: i32,
    workspaces: Vec<WorkspaceSession>,
    remote_uris: Vec<String>,
    remote_names: BTreeMap<String, String>,
    appearance: AppearanceMode,
    color_theme: ColorTheme,
    parallel_transfers: bool,
    workflow: WorkflowPreferences,
    custom_tools: Vec<CustomToolSession>,
    custom_tools_revision: u64,
    tags: BTreeMap<String, String>,
    tags_revision: u64,
    palette_open: bool,
    palette_query: String,
    palette_selection: usize,
    focus_location_epoch: u64,
    focus_sidebar_epoch: u64,
    focus_inspector_epoch: u64,
    vfs: Arc<dyn Vfs>,
    operation_engine: OperationEngine,
    active_operations: usize,
    operations: BTreeMap<JobId, OperationStatus>,
    operation_log: VecDeque<OperationLogEntry>,
    operation_log_revision: u64,
    next_operation_log_id: u64,
    pending_history: BTreeMap<JobId, HistoryEntry>,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    history_busy: bool,
    recovery_records: Vec<dualpane_engine::journal::RecoveryRecord>,
    recovery_errors: Vec<String>,
    clipboard_cut_jobs: BTreeMap<JobId, u64>,
    clipboard_provider: Option<gdk::ContentProvider>,
    clipboard_generation: u64,
    search_open: bool,
    search_content_preset: bool,
    search_generation: u64,
    search_cancel: Option<CancelToken>,
    search_loading: bool,
    search_results: SearchResults,
    search_error: Option<String>,
    search_session: Option<search_actions::Session>,
    tool_cancel: Option<CancelToken>,
    archive_mounts: archive_browser::ArchiveLocations,
    terminal_visible: bool,
    terminal_tabs: Vec<TerminalTabState>,
    active_terminal: Option<u64>,
    next_terminal_id: u64,
    terminal_revision: u64,
    session_worker: SessionWorker,
    started: Instant,
    profile_startup: bool,
    benchmark_mode: bool,
    benchmark_filter: Option<String>,
    startup_reported: bool,
    filter_benchmark_started: bool,
    filter_benchmark_scheduled: bool,
    filter_benchmark_started_at: [Option<Instant>; 2],
    filter_benchmark_results: [Option<Duration>; 2],
    filter_benchmark_worker_results: [Option<Duration>; 2],
    quit_after_first_paint: bool,
    first_paints: [Option<Duration>; 2],
    complete_paints: [Option<Duration>; 2],
    scroll_results: [Option<ScrollMetrics>; 2],
    scroll_epoch: u64,
    rss_pending: bool,
    thumbnail_scheduler: Option<ThumbnailScheduler>,
    thumbnail_bridge: Option<JoinHandle<()>>,
    thumbnail_request_id: u64,
    thumbnail_outstanding: HashMap<VPath, BTreeSet<u64>>,
    thumbnail_applied: HashMap<VPath, u64>,
    thumbnail_revision: u64,
    thumbnail_event: Option<ThumbnailUiEvent>,
    aux_workers: Vec<JoinHandle<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorTarget {
    First,
    Last,
    PageUp,
    PageDown,
}

#[derive(Clone, Debug)]
enum PaletteAction {
    Command(CommandId),
    Navigate(VPath),
    Workspace(usize),
    Remote(String),
    CustomTool(usize),
}

#[derive(Clone, Debug)]
struct PaletteItem {
    action: PaletteAction,
    label: String,
    binding: String,
}

/// Everything the UI needs once an engine job reaches a terminal state.
#[derive(Debug)]
pub struct FinishedOperation {
    pub backups: Vec<dualpane_engine::journal::BackupRecord>,
    pub completed_items: u64,
    pub id: JobId,
    pub source_pane: PaneId,
    pub kind: JobKind,
    pub state: JobState,
    pub errors: Vec<dualpane_engine::JobError>,
    pub trash_records: Vec<TrashRecord>,
    pub transfers: Vec<TransferRecord>,
}

#[derive(Debug)]
pub enum AppMsg {
    DevicesChanged,
    DeviceMountRemoved(VPath),
    OpenDevice(String),
    RemoveDevice(String),
    DeviceFinished(Result<devices::DeviceOutcome, String>),
    ShowRecovery,
    RecoveryReady {
        records: Vec<dualpane_engine::journal::RecoveryRecord>,
        errors: Vec<String>,
        show: bool,
    },
    RestoreMissingOriginals(std::path::PathBuf),
    RestoreArchive(std::path::PathBuf),
    ArchiveRestoreReady {
        source: VPath,
        result: Result<dualpane_engine::journal::ArchiveChange, String>,
    },
    RecoveryFinished(Result<String, String>),
    ReviewRecovery(std::path::PathBuf),
    ExecuteCommand(CommandId),
    NavigateActive(VPath),
    SidebarLocation {
        path: VPath,
        action: sidebar::menus::LocationAction,
    },
    RemoveRecent(VPath),
    ClearRecent,
    SetPaletteQuery(String),
    ClosePalette,
    OmarchyThemeChanged,
    MovePaletteSelection(i32),
    MovePaletteSelectionToEnd(bool),
    ActivatePaletteSelection,
    ActivatePaletteItem(usize),
    OpenGlob(bool),
    SetGlob(PaneId, String),
    ApplyGlob(PaneId),
    ActivatePane(PaneId),
    SwitchPane,
    BackActive,
    ForwardActive,
    UpActive,
    RefreshActive,
    NewTabActive,
    CloseTabActive,
    ToggleOrientation,
    ToggleDualPane,
    SetPaneFilter(PaneId, String),
    SetViewMode(PaneViewMode),
    SetSort(PaneId, SortKey),
    ToggleSortDirection(PaneId),
    ToggleHiddenActive,
    TogglePreview,
    ToggleQuickLook,
    OpenSearch(bool),
    CloseSearch,
    CancelSearch,
    RunSearch(SearchOptions),
    SearchReady {
        generation: u64,
        result: Result<SearchResults, String>,
    },
    SearchAction(search_actions::Request),
    SearchDeleteConfirmed(search_actions::Request),
    RefreshSearch,
    SyncReady(Result<usize, String>),
    ChecksumReady {
        path: VPath,
        result: Result<String, String>,
    },
    ChecksumsReady {
        left: VPath,
        right: VPath,
        result: Result<(String, String), String>,
    },
    ApplyPermissions {
        path: VPath,
        mode: u32,
        recursive: bool,
    },
    PermissionsInspected {
        pane: PaneId,
        path: VPath,
        result: Result<u32, String>,
    },
    PermissionsReady {
        path: VPath,
        mode: u32,
        recursive: bool,
        result: Result<usize, String>,
    },
    ApplyElevatedPermissions {
        path: VPath,
        mode: u32,
        recursive: bool,
    },
    ElevatedPermissionsReady(Result<(), String>),
    CreateArchive {
        name: String,
        format: ArchiveFormat,
        password: Option<crate::archive::Password>,
    },
    ArchivePasswordRequested(archive_password::Request),
    ArchiveProgress {
        id: JobId,
        progress: ArchiveProgress,
    },
    ArchiveReady {
        id: JobId,
        pane: PaneId,
        result: Result<usize, String>,
    },
    ArchiveBrowseReady {
        pane: PaneId,
        id: JobId,
        result: Result<archive_browser::OpenedArchive, String>,
    },
    ArchiveReloadReady {
        source: VPath,
        id: JobId,
        result: Result<archive_browser::OpenedArchive, String>,
    },
    ConvertImage(ImageOutputFormat),
    ImageConverted(Result<VPath, String>),
    RunPdfTool {
        options: PdfToolOptions,
        other_pane: bool,
    },
    PdfReady(Result<Vec<VPath>, String>),
    ToggleTerminal,
    NewTerminal,
    SelectTerminal(u64),
    CloseTerminal(u64),
    CloseAllTerminals,
    TerminalStarted {
        id: u64,
        result: Result<(TerminalSession, std::sync::mpsc::Receiver<TerminalEvent>), String>,
    },
    TerminalEvent(u64, TerminalEvent),
    TerminalInput(Vec<u8>),
    ResizeTerminal {
        id: u64,
        rows: u16,
        cols: u16,
    },
    SetInspectorPage(InspectorPage),
    ClearOperationLog,
    DismissOperation(JobId),
    RetryOperation(JobId),
    CreateDirectory(String),
    DirectoryCreated {
        pane: PaneId,
        result: Result<(), String>,
    },
    CreateFile(String),
    FileCreated {
        pane: PaneId,
        result: Result<(), String>,
    },
    RenamePath(VPath, String),
    RenameFinished {
        pane: PaneId,
        source: VPath,
        destination: VPath,
        result: Result<(), String>,
    },
    UpdateArchive {
        pane: PaneId,
        request: archive_actions::Request,
    },
    ArchiveUpdateReady {
        id: JobId,
        pane: PaneId,
        source: VPath,
        result: Result<Option<dualpane_engine::journal::ArchiveChange>, String>,
    },
    BatchRename(Vec<(VPath, String)>),
    BatchRenameFinished(Result<Vec<(VPath, VPath)>, String>),
    DeletePermanentConfirmed,
    SecureDeleteConfirmed {
        pane: PaneId,
        plan: dualpane_platform::secure_delete::SecureDeletePlan,
    },
    SecureDeleteProgress {
        id: JobId,
        progress: dualpane_platform::secure_delete::SecureDeleteProgress,
    },
    SecureDeleteReady {
        id: JobId,
        pane: PaneId,
        result: Result<usize, String>,
    },
    OpenFailed(PaneId, String),
    PreviewReady {
        generation: u64,
        path: VPath,
        result: Result<Preview, PreviewError>,
    },
    LoadPreview {
        generation: u64,
        path: VPath,
    },
    SelectionSummaryReady {
        generation: u64,
        summary: SelectionSummary,
    },
    FolderMeasureReady {
        generation: u64,
        result: Result<FolderMeasureResult, String>,
    },
    GitInfoReady {
        generation: u64,
        info: Option<GitInfo>,
    },
    RefreshPaneGit,
    PaneGitReady {
        pane: PaneId,
        path: VPath,
        info: Option<pane_git::GitStatus>,
    },
    RequestThumbnail {
        pane: PaneId,
        generation: u64,
        path: VPath,
    },
    ThumbnailReady(ThumbnailResponse),
    AppendFilter(char),
    ClearLayered,
    SelectionChanged(PaneId, Selection, Option<u32>),
    ContextTarget(PaneId, Option<(VPath, EntryKind)>),
    TabFolderAction {
        target: context_menu::TabFolderTarget,
        action: Box<AppMsg>,
    },
    PasteInto(PaneId, VPath),
    MoveCursor(i32, bool),
    MoveCursorVertical(i32, bool),
    MoveCursorHorizontal(i32, bool),
    MoveCursorTo(CursorTarget, bool),
    ToggleCursor,
    SelectAllActive,
    ClearSelectionActive,
    InvertSelectionActive,
    OpenCursor,
    Navigate(PaneId, VPath),
    NavigateExact(PaneId, VPath),
    CancelLocation(PaneId),
    Back(PaneId),
    Forward(PaneId),
    Up(PaneId),
    Refresh(PaneId),
    OpenRow(PaneId, u32),
    MillerOpen(PaneId, usize, u32),
    MillerSelectionChanged {
        pane: PaneId,
        column: usize,
        path: VPath,
        selection: Selection,
        row: Option<u32>,
    },
    ClipboardFiles {
        pane: PaneId,
        destination: VPath,
        result: Result<(Vec<VPath>, bool), String>,
        owner: Option<u64>,
    },
    MillerResize(PaneId, VPath, i32),
    NavigationScroll {
        pane: PaneId,
        path: VPath,
        x: bool,
        value: u32,
    },
    MillerRetry(PaneId, usize, VPath),
    MillerActivateFile(PaneId, VPath, EntryKind),
    MillerRefreshed {
        pane: PaneId,
        generation: u64,
        snapshots: Vec<(VPath, Result<model_miller::MillerSnapshot, IndexError>)>,
    },
    NewTab(PaneId),
    CloseTab(PaneId),
    CloseTabAt(PaneId, usize),
    SelectTab(PaneId, usize),
    SplitPosition(i32),
    PreviewWidth(i32),
    ResetPreviewWidth,
    WindowSize(i32, i32),
    SaveWorkspace(String),
    UpdateWorkspace(usize),
    RenameWorkspace {
        index: usize,
        name: String,
    },
    RemoveWorkspace(usize),
    OpenWorkspace(usize),
    MoveBookmark {
        from: usize,
        before: usize,
    },
    MoveBookmarkToEnd(usize),
    RemoveBookmark(usize),
    RenameFavorite {
        group: Option<usize>,
        path: VPath,
        name: String,
    },
    SetSidebarGroupExpanded {
        key: String,
        expanded: bool,
    },
    CreateFavoriteGroup(String),
    RemoveFavoriteGroup(usize),
    AddCurrentToFavoriteGroup(usize),
    RemoveGroupedFavorite {
        group: usize,
        item: usize,
    },
    ConnectRemote(String),
    RemoveRemote(String),
    SaveRemote {
        uri: String,
        replacing: String,
        name: String,
    },
    /// Reopens the Connect sheet with this address ready to edit; connecting
    /// replaces the old entry.
    EditRemote(String),
    /// Drops the keyring password for the address's server, then reconnects so
    /// the credential prompt comes up fresh.
    ForgetRemotePassword(String),
    ConnectRemoteWithOptions {
        connection: remote::RemoteConnection,
        replacing: Option<String>,
        name: Option<String>,
    },
    RemotePasswordForgotten {
        uri: String,
        result: Result<usize, String>,
    },
    RemoteConnected {
        uri: String,
        result: Result<VPath, String>,
    },
    SetSettings(Box<settings::SettingsDraft>),
    SettingsSaveFailed(String),
    ManageCustomTools,
    SetCustomTools(Vec<CustomToolSession>),
    RunCustomTool(usize),
    CustomToolFinished(Result<String, String>),
    DropFiles {
        sources: Vec<VPath>,
        destination: VPath,
        action: FileDropAction,
    },
    Listing {
        pane: PaneId,
        generation: u64,
        event: ListingEvent,
    },
    MetadataVisible(PaneId, u32),
    MeasureSelection {
        pane: PaneId,
        generation: u64,
    },
    SelectionSizeReady {
        pane: PaneId,
        generation: u64,
        result: Result<FolderMeasureResult, String>,
    },
    FilesystemChanged {
        path: VPath,
        watch_id: u64,
        batch: dualpane_index::WatchBatch,
    },
    FilesystemUpdated(model_watch::LiveResult),
    MetadataReady {
        pane: PaneId,
        generation: u64,
        result: Result<MetadataResult, IndexError>,
    },
    FilterReady {
        pane: PaneId,
        generation: u64,
        queue_delay: Duration,
        result: Result<FilterResult, IndexError>,
    },
    FilterWorkerReady(PaneId),
    StartFilterBenchmark,
    GlobReady {
        pane: PaneId,
        generation: u64,
        select: bool,
        keys: Vec<SelectionKey>,
        elapsed: Duration,
    },
    FramePainted {
        pane: PaneId,
        complete: bool,
        elapsed: Duration,
    },
    ScrollFinished(PaneId, ScrollMetrics),
    RssMeasured(Option<u64>),
    OperationFinished(FinishedOperation),
    OperationEvent {
        kind: JobKind,
        event: JobEvent,
    },
    ResolveConflict {
        job_id: JobId,
        conflict_id: ConflictId,
        choice: ConflictChoice,
        apply_to_all: bool,
    },
    CompareConflictChecksum {
        pane: PaneId,
        job_id: JobId,
        conflict_id: ConflictId,
        source: VPath,
        destination: VPath,
    },
    CancelFirstOperation,
    TogglePauseFirstOperation,
    CancelOperation(JobId),
    TogglePauseOperation(JobId),
    HistoryFinished {
        entry: HistoryEntry,
        direction: HistoryDirection,
        result: Result<(), String>,
    },
}

pub struct AppWidgets {
    paned: gtk::Paned,
    workspace_paned: gtk::Paned,
    sidebar_revealer: gtk::Revealer,
    sidebar_focus_target: gtk::Button,
    sidebar_groups: Vec<sidebar::CollapsibleGroup>,
    sidebar_favorite_groups: Vec<sidebar::CollapsibleGroup>,
    sidebar_bookmarks: gtk::Box,
    sidebar_recent: gtk::Box,
    sidebar_workspaces: gtk::Box,
    sidebar_remotes: gtk::Box,
    sidebar_devices: devices::DeviceSidebar,
    sidebar_places: Vec<(VPath, gtk::Button)>,
    sidebar_bookmark_rows: Vec<(VPath, gtk::Button)>,
    sidebar_recent_rows: Vec<(VPath, gtk::Button)>,
    sidebar_remote_rows: Vec<(VPath, gtk::Button)>,
    rendered_active_location: Option<VPath>,
    rendered_sidebar_visible: bool,
    rendered_focus_sidebar_epoch: u64,
    rendered_focus_inspector_epoch: u64,
    rendered_preview_visible: bool,
    rendered_preview_width: i32,
    rendered_bookmarks: Option<widgets::SidebarFavorites>,
    rendered_recent: Vec<String>,
    rendered_workspaces: Vec<String>,
    rendered_remotes: Vec<String>,
    rendered_remote_names: BTreeMap<String, String>,
    rendered_remote_devices: Option<u64>,
    palette_dialog: adw::Dialog,
    palette_parent: adw::ApplicationWindow,
    palette_entry: gtk::SearchEntry,
    palette_scroll: gtk::ScrolledWindow,
    palette_commands: gtk::Box,
    palette_presented: Rc<Cell<bool>>,
    palette_input_active: Rc<Cell<bool>>,
    palette_pending_open: Rc<Cell<bool>>,
    rendered_palette: Option<(String, KeymapProfile, usize, action_policy::Context)>,
    topbar: topbar::TopBarWidgets,
    global_search: gtk::SearchEntry,
    rendered_dual_pane: bool,
    rendered_focus_filter_epoch: u64,
    rendered_focus_location_epoch: u64,
    jobs: job_view::JobActivityWidgets,
    preview: PreviewWidgets,
    quick_look: QuickLookWidgets,
    search: SearchWidgets,
    terminal: TerminalWidgets,
    tag_store: Rc<RefCell<BTreeMap<String, String>>>,
    custom_tool_store: Rc<RefCell<Vec<CustomToolSession>>>,
    rendered_tags_revision: u64,
    rendered_custom_tools_revision: u64,
    thumbnail_ui: Rc<RefCell<ThumbnailUiState>>,
    rendered_thumbnail_revision: u64,
    file_drag_ui: Rc<FileDragUiState>,
    panes: [PaneWidgets; 2],
    rendered_scroll_epoch: u64,
    _omarchy_theme_monitor: Option<gio::FileMonitor>,
}

struct PaletteKeyboardState {
    input_active: Rc<Cell<bool>>,
    pending_open: Rc<Cell<bool>>,
    presented: Rc<Cell<bool>>,
    entry: gtk::SearchEntry,
    dialog: adw::Dialog,
}

impl AppModel {
    fn pane(&self, pane: PaneId) -> &PaneState {
        &self.panes[pane.index()]
    }

    fn pane_mut(&mut self, pane: PaneId) -> &mut PaneState {
        &mut self.panes[pane.index()]
    }

    fn focus_active_files(&mut self) {
        let pane = self.active_pane;
        let state = self.pane_mut(pane);
        state.focus_files_epoch = state.focus_files_epoch.wrapping_add(1);
    }

    fn workspace_snapshot(&self, name: &str) -> WorkspaceSession {
        WorkspaceSession {
            name: name.to_owned(),
            left: self
                .archive_mounts
                .display(&self.pane(PaneId::Left).active().path)
                .to_string(),
            right: self
                .archive_mounts
                .display(&self.pane(PaneId::Right).active().path)
                .to_string(),
            left_pane: Some(
                self.archive_mounts
                    .session(self.pane(PaneId::Left).to_session()),
            ),
            right_pane: Some(
                self.archive_mounts
                    .session(self.pane(PaneId::Right).to_session()),
            ),
            active_pane: Some(self.active_pane.index() as u8),
            dual_pane: Some(self.dual_pane),
            vertical_split: Some(self.vertical_split),
            split_position: Some(self.split_position),
            sidebar_visible: Some(self.sidebar_visible),
            preview_visible: Some(self.preview_visible),
            preview_width: Some(self.preview_width),
        }
    }

    fn refresh_grid_column_estimates(&mut self) {
        let reserved_width = i32::from(self.sidebar_visible) * SIDEBAR_WIDTH
            + i32::from(self.preview_visible) * self.preview_width;
        let available_width = self.window_width.saturating_sub(reserved_width).max(1);
        let responsive_vertical = available_width < 840;
        let pane_width = if self.vertical_split || responsive_vertical {
            available_width
        } else {
            available_width / 2
        };
        let columns = estimated_grid_columns(pane_width);
        for pane in &mut self.panes {
            pane.grid_columns = columns;
        }
    }

    fn palette_items(&self) -> Vec<PaletteItem> {
        let query = self.palette_query.as_str();
        let context = self.action_context(self.active_pane);
        let mut items = matching_commands("")
            .into_iter()
            .filter(|definition| {
                let action = context.action(definition.id);
                action.visible
                    && palette_query_matches(
                        query,
                        &[
                            action.label(definition.label),
                            definition.label,
                            definition.id.as_str(),
                        ],
                    )
            })
            .map(|definition| PaletteItem {
                action: PaletteAction::Command(definition.id),
                label: context
                    .action(definition.id)
                    .label(definition.label)
                    .to_owned(),
                binding: self.keymap.binding_label(definition.id),
            })
            .collect::<Vec<_>>();
        items.extend(self.bookmarks.iter().filter_map(|path| {
            let label = self.bookmark_labels.get(&path.to_string()).map_or_else(
                || format!("Open favorite · {path}"),
                |name| format!("Open favorite · {name} · {path}"),
            );
            palette_query_matches(query, &[&label, "favorite bookmark open"]).then(|| PaletteItem {
                action: PaletteAction::Navigate(path.clone()),
                label,
                binding: String::new(),
            })
        }));
        for group in &self.favorite_groups {
            items.extend(group.paths.iter().filter_map(|path| {
                let label = group.labels.get(path).map_or_else(
                    || format!("Open {} favorite · {path}", group.name),
                    |name| format!("Open {} favorite · {name} · {path}", group.name),
                );
                palette_query_matches(query, &[&label, "favorite group bookmark open"]).then(|| {
                    PaletteItem {
                        action: PaletteAction::Navigate(VPath::from(path.as_str())),
                        label,
                        binding: String::new(),
                    }
                })
            }));
        }
        items.extend(self.recent.iter().filter_map(|path| {
            let label = format!("Open recent · {path}");
            palette_query_matches(query, &[&label, "history recent open"]).then(|| PaletteItem {
                action: PaletteAction::Navigate(path.clone()),
                label,
                binding: String::new(),
            })
        }));
        items.extend(
            self.workspaces
                .iter()
                .enumerate()
                .filter_map(|(index, workspace)| {
                    let label = format!("Open workspace · {}", workspace.name);
                    palette_query_matches(query, &[&label, "workspace open"]).then(|| PaletteItem {
                        action: PaletteAction::Workspace(index),
                        label,
                        binding: String::new(),
                    })
                }),
        );
        items.extend(self.remote_uris.iter().filter_map(|uri| {
            let name = self.remote_names.get(uri).unwrap_or(uri);
            let label = format!("Connect remote · {name}");
            palette_query_matches(query, &[&label, uri, "server remote connect"]).then(|| {
                PaletteItem {
                    action: PaletteAction::Remote(uri.clone()),
                    label,
                    binding: String::new(),
                }
            })
        }));
        items.extend(
            self.custom_tools
                .iter()
                .enumerate()
                .filter_map(|(index, tool)| {
                    let label = format!("Run custom tool · {}", tool.name);
                    (tool.enabled
                        && context.custom_tool_reason().is_none()
                        && palette_query_matches(query, &[&label, "custom tool run"]))
                    .then(|| PaletteItem {
                        action: PaletteAction::CustomTool(index),
                        label,
                        binding: String::new(),
                    })
                }),
        );
        items
    }

    fn activate_palette_selection(&mut self, sender: &ComponentSender<Self>) {
        let Some(action) = self
            .palette_items()
            .get(self.palette_selection)
            .map(|item| item.action.clone())
        else {
            return;
        };
        match action {
            PaletteAction::Command(command) => self.execute_command(command, sender),
            PaletteAction::Navigate(path) => self.navigate(self.active_pane, path, sender),
            PaletteAction::Workspace(index) => {
                let _ = sender.input_sender().send(AppMsg::OpenWorkspace(index));
            }
            PaletteAction::Remote(uri) => {
                let _ = sender.input_sender().send(AppMsg::ConnectRemote(uri));
            }
            PaletteAction::CustomTool(index) => {
                let _ = sender.input_sender().send(AppMsg::RunCustomTool(index));
            }
        }
        self.palette_open = false;
        self.palette_query.clear();
        self.palette_selection = 0;
    }

    fn set_focused_tag(&mut self, color: Option<&str>) {
        let paths = self.operation_sources(self.active_pane);
        if paths.is_empty() {
            self.pane_mut(self.active_pane).error =
                Some("Select one or more items before tagging them".to_owned());
            return;
        }
        for path in paths {
            let key = path.to_string();
            if let Some(color) = color {
                self.tags.insert(key, color.to_owned());
            } else {
                self.tags.remove(&key);
            }
        }
        self.tags_revision = self.tags_revision.wrapping_add(1);
        self.persist_session();
    }
}

impl Drop for AppModel {
    fn drop(&mut self) {
        self.live_updates.stop();
        self.persist_session();
        for operation in self.operations.values() {
            if operation.is_active() {
                operation.control.cancel();
            }
        }
        if let Some(cancel) = self.tool_cancel.take() {
            cancel.cancel();
        }
        self.terminal_tabs.clear();
        self.archive_mounts.cancel_reloads();
        for pane in &mut self.panes {
            pane.archive_browse.cancel();
        }
        for pane in &self.panes {
            pane.git.cancel();
            pane.thumbnail_cancel.cancel();
        }
        self.thumbnail_scheduler.take();
        if let Some(bridge) = self.thumbnail_bridge.take() {
            let _ = bridge.join();
        }
        for worker in self.aux_workers.drain(..) {
            let _ = worker.join();
        }
    }
}
