use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::gdk;
use serde::{Deserialize, Serialize};

const DEFAULT_KEYMAPS: &str = include_str!("../assets/keymaps.toml");

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeymapProfile {
    #[default]
    Classic,
    Modern,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct KeymapOverrides {
    #[serde(default)]
    pub classic: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub modern: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandId {
    SwitchPane,
    Open,
    Parent,
    Back,
    Forward,
    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    CursorFirst,
    CursorLast,
    CursorPageUp,
    CursorPageDown,
    ToggleSelection,
    SelectAll,
    ClearSelection,
    InvertSelection,
    SelectGlob,
    DeselectGlob,
    FocusFilter,
    FocusLocation,
    ClearLayered,
    NewTab,
    CloseTab,
    PreviousTab,
    NextTab,
    Bookmark,
    NewFavoriteGroup,
    Refresh,
    MatchOtherPane,
    SwapPanes,
    ToggleOrientation,
    ToggleDualPane,
    ToggleSidebar,
    FocusSidebar,
    TogglePreview,
    FocusInspector,
    QuickLook,
    ToggleHidden,
    FocusFiles,
    CommandPalette,
    ClassicKeymap,
    ModernKeymap,
    ViewFile,
    EditFile,
    Rename,
    BatchRename,
    OpenWith,
    OpenInNewTab,
    OpenOtherPane,
    Reveal,
    CopyPath,
    CopyDirectoryPath,
    CopyClipboard,
    Cut,
    Paste,
    Copy,
    Move,
    NewFile,
    NewDirectory,
    Trash,
    DeletePermanent,
    SecureDelete,
    Undo,
    Redo,
    RecursiveSearch,
    UnifiedSearch,
    Checksum,
    Permissions,
    CreateArchive,
    ExtractArchive,
    BrowseArchive,
    ConvertImage,
    PdfTools,
    ToggleTerminal,
    TogglePauseFirstOperation,
    CancelFirstOperation,
    InspectorInfo,
    InspectorWork,
    InspectorLog,
    SaveWorkspace,
    ConnectRemote,
    Settings,
    ShortcutReference,
    TagRed,
    TagOrange,
    TagYellow,
    TagGreen,
    TagBlue,
    TagPurple,
    ClearTag,
    DetailsView,
    CompactView,
    IconsView,
    SortByName,
    SortBySize,
    SortByModified,
    SortByType,
    ToggleSortDirection,
    CompareDirectories,
    FindDuplicates,
    CompareFiles,
    EditArchive,
}

impl CommandId {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SwitchPane => "switch-pane",
            Self::Open => "open",
            Self::Parent => "parent",
            Self::Back => "back",
            Self::Forward => "forward",
            Self::CursorUp => "cursor-up",
            Self::CursorDown => "cursor-down",
            Self::CursorLeft => "cursor-left",
            Self::CursorRight => "cursor-right",
            Self::CursorFirst => "cursor-first",
            Self::CursorLast => "cursor-last",
            Self::CursorPageUp => "cursor-page-up",
            Self::CursorPageDown => "cursor-page-down",
            Self::ToggleSelection => "toggle-selection",
            Self::SelectAll => "select-all",
            Self::ClearSelection => "clear-selection",
            Self::InvertSelection => "invert-selection",
            Self::SelectGlob => "select-glob",
            Self::DeselectGlob => "deselect-glob",
            Self::FocusFilter => "focus-filter",
            Self::FocusLocation => "focus-location",
            Self::ClearLayered => "clear-layered",
            Self::NewTab => "new-tab",
            Self::CloseTab => "close-tab",
            Self::PreviousTab => "previous-tab",
            Self::NextTab => "next-tab",
            Self::Bookmark => "bookmark",
            Self::NewFavoriteGroup => "new-favorite-group",
            Self::Refresh => "refresh",
            Self::MatchOtherPane => "match-other-pane",
            Self::SwapPanes => "swap-panes",
            Self::ToggleOrientation => "toggle-orientation",
            Self::ToggleDualPane => "toggle-dual-pane",
            Self::ToggleSidebar => "toggle-sidebar",
            Self::FocusSidebar => "focus-sidebar",
            Self::TogglePreview => "toggle-preview",
            Self::FocusInspector => "focus-inspector",
            Self::QuickLook => "quick-look",
            Self::ToggleHidden => "toggle-hidden",
            Self::FocusFiles => "focus-files",
            Self::CommandPalette => "command-palette",
            Self::ClassicKeymap => "classic-keymap",
            Self::ModernKeymap => "modern-keymap",
            Self::ViewFile => "view-file",
            Self::EditFile => "edit-file",
            Self::Rename => "rename",
            Self::BatchRename => "batch-rename",
            Self::OpenWith => "open-with",
            Self::OpenInNewTab => "open-in-new-tab",
            Self::OpenOtherPane => "open-other-pane",
            Self::Reveal => "reveal",
            Self::CopyPath => "copy-path",
            Self::CopyDirectoryPath => "copy-directory-path",
            Self::CopyClipboard => "copy-clipboard",
            Self::Cut => "cut",
            Self::Paste => "paste",
            Self::Copy => "copy",
            Self::Move => "move",
            Self::NewFile => "new-file",
            Self::NewDirectory => "new-directory",
            Self::Trash => "trash",
            Self::DeletePermanent => "delete-permanent",
            Self::SecureDelete => "secure-delete",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::RecursiveSearch => "recursive-search",
            Self::UnifiedSearch => "unified-search",
            Self::Checksum => "checksum",
            Self::Permissions => "permissions",
            Self::CreateArchive => "create-archive",
            Self::ExtractArchive => "extract-archive",
            Self::BrowseArchive => "browse-archive",
            Self::ConvertImage => "convert-image",
            Self::PdfTools => "pdf-tools",
            Self::ToggleTerminal => "toggle-terminal",
            Self::TogglePauseFirstOperation => "toggle-pause-first-operation",
            Self::CancelFirstOperation => "cancel-first-operation",
            Self::InspectorInfo => "inspector-info",
            Self::InspectorWork => "inspector-work",
            Self::InspectorLog => "inspector-log",
            Self::SaveWorkspace => "save-workspace",
            Self::ConnectRemote => "connect-remote",
            Self::Settings => "settings",
            Self::ShortcutReference => "shortcut-reference",
            Self::TagRed => "tag-red",
            Self::TagOrange => "tag-orange",
            Self::TagYellow => "tag-yellow",
            Self::TagGreen => "tag-green",
            Self::TagBlue => "tag-blue",
            Self::TagPurple => "tag-purple",
            Self::ClearTag => "clear-tag",
            Self::DetailsView => "details-view",
            Self::CompactView => "compact-view",
            Self::IconsView => "icons-view",
            Self::SortByName => "sort-by-name",
            Self::SortBySize => "sort-by-size",
            Self::SortByModified => "sort-by-modified",
            Self::SortByType => "sort-by-type",
            Self::ToggleSortDirection => "toggle-sort-direction",
            Self::FindDuplicates => "find-duplicates",
            Self::CompareFiles => "compare-files",
            Self::EditArchive => "edit-archive",
            Self::CompareDirectories => "compare-directories",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CommandDefinition {
    pub id: CommandId,
    pub label: &'static str,
    pub available: bool,
}

pub const COMMANDS: &[CommandDefinition] = &[
    command(CommandId::SwitchPane, "Switch active pane", true),
    command(CommandId::Open, "Open or descend", true),
    command(CommandId::Parent, "Parent directory", true),
    command(CommandId::Back, "History back", true),
    command(CommandId::Forward, "History forward", true),
    command(CommandId::CursorUp, "Move cursor up", true),
    command(CommandId::CursorDown, "Move cursor down", true),
    command(CommandId::CursorLeft, "Move cursor left", true),
    command(CommandId::CursorRight, "Move cursor right", true),
    command(CommandId::CursorFirst, "Move cursor to first item", true),
    command(CommandId::CursorLast, "Move cursor to last item", true),
    command(CommandId::CursorPageUp, "Move cursor one page up", true),
    command(CommandId::CursorPageDown, "Move cursor one page down", true),
    command(CommandId::ToggleSelection, "Toggle selection", true),
    command(CommandId::SelectAll, "Select all", true),
    command(CommandId::ClearSelection, "Clear selection", true),
    command(CommandId::InvertSelection, "Invert selection", true),
    command(CommandId::SelectGlob, "Select by glob", true),
    command(CommandId::DeselectGlob, "Deselect by glob", true),
    command(CommandId::FocusFilter, "Focus fuzzy filter", true),
    command(CommandId::FocusLocation, "Edit location", true),
    command(
        CommandId::ClearLayered,
        "Clear filter, selection, or panel",
        true,
    ),
    command(CommandId::NewTab, "New tab", true),
    command(CommandId::CloseTab, "Close tab", true),
    command(CommandId::PreviousTab, "Previous tab", true),
    command(CommandId::NextTab, "Next tab", true),
    command(CommandId::Bookmark, "Bookmark current directory", true),
    command(CommandId::NewFavoriteGroup, "New favorite group", true),
    command(CommandId::Refresh, "Force refresh", true),
    command(
        CommandId::MatchOtherPane,
        "Match other pane directory",
        true,
    ),
    command(CommandId::SwapPanes, "Swap panes", true),
    command(
        CommandId::ToggleOrientation,
        "Toggle split orientation",
        true,
    ),
    command(
        CommandId::ToggleDualPane,
        "Toggle one or two file panels",
        true,
    ),
    command(CommandId::ToggleSidebar, "Toggle sidebar", true),
    command(CommandId::FocusSidebar, "Focus locations sidebar", true),
    command(CommandId::TogglePreview, "Toggle inspector", true),
    command(CommandId::FocusInspector, "Focus inspector", true),
    command(CommandId::QuickLook, "Quick Look", true),
    command(CommandId::ToggleHidden, "Toggle hidden files", true),
    command(CommandId::FocusFiles, "Focus active file pane", true),
    command(CommandId::CommandPalette, "Command palette", true),
    command(CommandId::ClassicKeymap, "Use classic keymap", true),
    command(CommandId::ModernKeymap, "Use modern keymap", true),
    command(CommandId::ViewFile, "View file", true),
    command(CommandId::EditFile, "Edit file", true),
    command(CommandId::Rename, "Rename", true),
    command(CommandId::BatchRename, "Batch rename selection", true),
    command(CommandId::OpenWith, "Open with application", true),
    command(CommandId::OpenInNewTab, "Open folder in new tab", true),
    command(CommandId::OpenOtherPane, "Open in other pane", true),
    command(CommandId::Reveal, "Reveal in system file manager", true),
    command(CommandId::CopyPath, "Copy path", true),
    command(
        CommandId::CopyDirectoryPath,
        "Copy current folder path",
        true,
    ),
    command(CommandId::CopyClipboard, "Copy to file clipboard", true),
    command(CommandId::Cut, "Cut to file clipboard", true),
    command(CommandId::Paste, "Paste file clipboard", true),
    command(CommandId::Copy, "Copy to other pane", true),
    command(CommandId::Move, "Move or rename", true),
    command(CommandId::NewFile, "New file", true),
    command(CommandId::NewDirectory, "New directory", true),
    command(CommandId::Trash, "Move to trash", true),
    command(CommandId::DeletePermanent, "Delete permanently", true),
    command(CommandId::SecureDelete, "Secure delete files…", true),
    command(CommandId::Undo, "Undo last file operation", true),
    command(CommandId::Redo, "Redo last file operation", true),
    command(CommandId::RecursiveSearch, "Search file contents", true),
    command(CommandId::UnifiedSearch, "Search names and paths", true),
    command(CommandId::Checksum, "Compute SHA-256 checksum", true),
    command(CommandId::Permissions, "Edit permissions", true),
    command(CommandId::CreateArchive, "Create archive", true),
    command(CommandId::ExtractArchive, "Extract archive", true),
    command(CommandId::BrowseArchive, "Browse archive", true),
    command(CommandId::ConvertImage, "Convert image", true),
    command(CommandId::PdfTools, "PDF tools", true),
    command(CommandId::ToggleTerminal, "Toggle terminal", true),
    command(
        CommandId::TogglePauseFirstOperation,
        "Pause or resume first background task",
        true,
    ),
    command(
        CommandId::CancelFirstOperation,
        "Cancel first background task",
        true,
    ),
    command(CommandId::InspectorInfo, "Show inspector information", true),
    command(CommandId::InspectorWork, "Show background tasks", true),
    command(CommandId::InspectorLog, "Show activity log", true),
    command(CommandId::SaveWorkspace, "Save current workspace", true),
    command(CommandId::ConnectRemote, "Connect to remote storage", true),
    command(CommandId::Settings, "Settings", true),
    command(CommandId::ShortcutReference, "Keyboard shortcuts", true),
    command(CommandId::TagRed, "Tag selected items red", true),
    command(CommandId::TagOrange, "Tag selected items orange", true),
    command(CommandId::TagYellow, "Tag selected items yellow", true),
    command(CommandId::TagGreen, "Tag selected items green", true),
    command(CommandId::TagBlue, "Tag selected items blue", true),
    command(CommandId::TagPurple, "Tag selected items purple", true),
    command(CommandId::ClearTag, "Clear selected item tags", true),
    command(CommandId::DetailsView, "Details view", true),
    command(CommandId::CompactView, "Column view", true),
    command(CommandId::IconsView, "Grid view", true),
    command(CommandId::SortByName, "Sort by name", true),
    command(CommandId::SortBySize, "Sort by size", true),
    command(CommandId::SortByModified, "Sort by modified date", true),
    command(CommandId::SortByType, "Sort by type", true),
    command(
        CommandId::ToggleSortDirection,
        "Reverse sort direction",
        true,
    ),
    command(CommandId::FindDuplicates, "Find duplicate files", true),
    command(CommandId::CompareFiles, "Compare files by contents", true),
    command(CommandId::EditArchive, "Edit archive contents", true),
    command(CommandId::CompareDirectories, "Compare directories", true),
];

const fn command(id: CommandId, label: &'static str, available: bool) -> CommandDefinition {
    CommandDefinition {
        id,
        label,
        available,
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct KeyStroke {
    key: gdk::Key,
    modifiers: gdk::ModifierType,
}

#[derive(Clone)]
pub struct Keymap {
    inner: Rc<RefCell<KeymapState>>,
}

struct KeymapState {
    profile: KeymapProfile,
    overrides: KeymapOverrides,
    source: KeymapOverrides,
    bindings: HashMap<CommandId, Vec<(KeyStroke, String)>>,
}

impl Keymap {
    #[must_use]
    pub fn new(profile: KeymapProfile, overrides: KeymapOverrides) -> Self {
        let mut source: KeymapOverrides =
            toml_edit::de::from_str(DEFAULT_KEYMAPS).expect("bundled keymaps.toml must be valid");
        source.classic.extend(overrides.classic.clone());
        source.modern.extend(overrides.modern.clone());
        let state = KeymapState {
            profile,
            overrides,
            source,
            bindings: HashMap::new(),
        };
        let keymap = Self {
            inner: Rc::new(RefCell::new(state)),
        };
        keymap.rebuild();
        keymap
    }

    #[must_use]
    pub fn profile(&self) -> KeymapProfile {
        self.inner.borrow().profile
    }

    pub fn set_profile(&self, profile: KeymapProfile) {
        if self.profile() != profile {
            self.inner.borrow_mut().profile = profile;
            self.rebuild();
        }
    }

    #[must_use]
    pub fn command_for(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> Option<CommandId> {
        let stroke = normalized_stroke(key, modifiers);
        let state = self.inner.borrow();
        COMMANDS.iter().find_map(|definition| {
            state
                .bindings
                .get(&definition.id)
                .into_iter()
                .flatten()
                .any(|(binding, _)| *binding == stroke)
                .then_some(definition.id)
        })
    }

    #[must_use]
    pub fn binding_label(&self, command: CommandId) -> String {
        self.inner
            .borrow()
            .bindings
            .get(&command)
            .map(|bindings| {
                bindings
                    .iter()
                    .map(|(_, label)| label.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .unwrap_or_default()
    }

    fn rebuild(&self) {
        let mut state = self.inner.borrow_mut();
        let source = match state.profile {
            KeymapProfile::Classic => &state.source.classic,
            KeymapProfile::Modern => &state.source.modern,
        };
        let mut bindings = HashMap::new();
        for definition in COMMANDS {
            let parsed = source
                .get(definition.id.as_str())
                .into_iter()
                .flatten()
                .filter_map(|accelerator| {
                    let Some((key, modifiers)) = gtk::accelerator_parse(accelerator) else {
                        tracing::warn!(command = definition.id.as_str(), %accelerator, "ignoring invalid key binding");
                        return None;
                    };
                    Some((
                        normalized_stroke(key, modifiers),
                        gtk::accelerator_get_label(key, modifiers).to_string(),
                    ))
                })
                .collect();
            bindings.insert(definition.id, parsed);
        }
        state.bindings = bindings;
    }

    pub fn overrides(&self) -> KeymapOverrides {
        self.inner.borrow().overrides.clone()
    }

    pub fn configure(&self, profile: KeymapProfile, overrides: KeymapOverrides) {
        let replacement = Self::new(profile, overrides);
        std::mem::swap(
            &mut *self.inner.borrow_mut(),
            &mut *replacement.inner.borrow_mut(),
        );
    }

    pub fn accelerators(&self, command: CommandId) -> Vec<String> {
        let state = self.inner.borrow();
        let source = match state.profile {
            KeymapProfile::Classic => &state.source.classic,
            KeymapProfile::Modern => &state.source.modern,
        };
        source.get(command.as_str()).cloned().unwrap_or_default()
    }

    /// Assign a shortcut and remove only that stroke from conflicting commands.
    pub fn assign(&self, command: CommandId, accelerator: Option<&str>) {
        let mut overrides = self.overrides();
        let source = match self.profile() {
            KeymapProfile::Classic => &mut overrides.classic,
            KeymapProfile::Modern => &mut overrides.modern,
        };
        if let Some((key, modifiers)) = accelerator.and_then(gtk::accelerator_parse) {
            let stroke = normalized_stroke(key, modifiers);
            for definition in COMMANDS {
                if definition.id == command {
                    continue;
                }
                let previous = self.accelerators(definition.id);
                let retained: Vec<_> = previous
                    .iter()
                    .filter(|value| {
                        gtk::accelerator_parse(*value).is_none_or(|(key, modifiers)| {
                            normalized_stroke(key, modifiers) != stroke
                        })
                    })
                    .cloned()
                    .collect();
                if retained != previous {
                    source.insert(definition.id.as_str().to_owned(), retained);
                }
            }
        }
        source.insert(
            command.as_str().to_owned(),
            accelerator.into_iter().map(str::to_owned).collect(),
        );
        self.configure(self.profile(), overrides);
    }
}

fn normalized_modifiers(modifiers: gdk::ModifierType) -> gdk::ModifierType {
    modifiers
        & (gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::SHIFT_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::META_MASK)
}

fn normalized_stroke(key: gdk::Key, modifiers: gdk::ModifierType) -> KeyStroke {
    let key = key.to_lower();
    let mut modifiers = normalized_modifiers(modifiers);
    if matches!(key, gdk::Key::plus | gdk::Key::asterisk) {
        modifiers.remove(gdk::ModifierType::SHIFT_MASK);
    }
    KeyStroke { key, modifiers }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use relm4::gtk::gdk;

    use super::{CommandId, DEFAULT_KEYMAPS, KeymapOverrides, KeymapProfile, normalized_stroke};

    #[test]
    fn user_override_toml_is_deserializable() {
        let overrides: KeymapOverrides = toml_edit::de::from_str(
            r#"
                [classic]
                switch-pane = ["<Control>Tab"]
                [modern]
                refresh = ["F5"]
            "#,
        )
        .expect("override");

        assert_eq!(
            overrides.classic[CommandId::SwitchPane.as_str()][0],
            "<Control>Tab"
        );
        assert_eq!(overrides.modern[CommandId::Refresh.as_str()][0], "F5");
        assert_eq!(KeymapProfile::default(), KeymapProfile::Classic);
    }

    #[test]
    fn shifted_symbols_do_not_require_a_duplicate_binding() {
        let stroke = normalized_stroke(gdk::Key::plus, gdk::ModifierType::SHIFT_MASK);
        assert_eq!(stroke.key, gdk::Key::plus);
        assert!(stroke.modifiers.is_empty());
    }

    #[test]
    fn bundled_profiles_have_no_ambiguous_shortcuts() {
        let keymaps: KeymapOverrides =
            toml_edit::de::from_str(DEFAULT_KEYMAPS).expect("bundled keymaps");
        for (profile, bindings) in [
            (KeymapProfile::Classic, keymaps.classic),
            (KeymapProfile::Modern, keymaps.modern),
        ] {
            let mut owners = HashMap::new();
            for (command, shortcuts) in bindings {
                for shortcut in shortcuts {
                    let normalized = shortcut.to_ascii_lowercase();
                    assert_eq!(
                        owners.insert(normalized, command.clone()),
                        None,
                        "{shortcut} is assigned to more than one command in {profile:?}"
                    );
                }
            }
        }
    }
}
