//! Shared availability and wording for commands, menus, shortcuts, and transfers.
use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Location {
    #[default]
    Folder,
    Archive,
    ReadOnlyArchive,
}

impl Location {
    pub(super) fn from_archive(access: Option<bool>) -> Self {
        match access {
            None => Self::Folder,
            Some(true) => Self::Archive,
            Some(false) => Self::ReadOnlyArchive,
        }
    }
    pub(super) fn is_archive(self) -> bool {
        self != Self::Folder
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Context {
    pub location: Location,
    pub destination: Location,
    pub items: usize,
    pub history_busy: bool,
    pub operations_active: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub can_parent: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Decision {
    pub visible: bool,
    pub reason: Option<&'static str>,
    label: Option<&'static str>,
    pub notify: bool,
}
impl Decision {
    pub(super) fn enabled(self) -> bool {
        self.reason.is_none()
    }
    pub(super) fn label(self, fallback: &'static str) -> &'static str {
        self.label.unwrap_or(fallback)
    }
}

pub(super) fn transfer(
    source: Location,
    destination: Location,
    moving: bool,
) -> Result<(), &'static str> {
    if destination == Location::ReadOnlyArchive {
        return Err("This archive is read-only. Copy the items to a regular folder instead.");
    }
    if moving && (source.is_archive() || destination.is_archive()) {
        return Err(
            "Moving across archive boundaries is not supported. Copy the items first, then remove the originals.",
        );
    }
    Ok(())
}

impl Context {
    pub(super) fn custom_tool_reason(self) -> Option<&'static str> {
        self.action(CommandId::EditFile).reason
    }
    pub(super) fn action(self, command: CommandId) -> Decision {
        use CommandId::*;
        let mut decision = Decision {
            visible: true,
            reason: None,
            label: None,
            notify: !matches!(command, Back | Forward | Parent),
        };
        let mutation = matches!(
            command,
            Rename
                | BatchRename
                | Cut
                | Move
                | Copy
                | Paste
                | Trash
                | DeletePermanent
                | SecureDelete
                | NewFile
                | NewDirectory
                | Permissions
                | ConvertImage
                | PdfTools
                | CreateArchive
                | ExtractArchive
                | EditFile
        );
        if self.location.is_archive() {
            if matches!(command, Trash | DeletePermanent) {
                decision.label = Some("Remove from archive…");
            }
            if matches!(
                command,
                Cut | Move
                    | SecureDelete
                    | Permissions
                    | ConvertImage
                    | PdfTools
                    | CreateArchive
                    | EditFile
                    | BatchRename
                    | OpenWith
            ) {
                decision.visible = false;
                decision.reason = Some(
                    "This action is not available inside archives. Copy the items to a regular folder first.",
                );
                return decision;
            }
            // Shift+Delete is an alias of archive removal, not a second menu action.
            if command == DeletePermanent {
                decision.visible = false;
            }
            if self.location == Location::ReadOnlyArchive
                && matches!(
                    command,
                    Trash | DeletePermanent | Rename | NewDirectory | NewFile | Paste
                )
            {
                decision.visible = false;
                decision.reason =
                    Some("This archive is read-only. Copy it to a regular folder to edit it.");
                return decision;
            }
        }
        decision.reason = if command == Back && !self.can_back {
            Some("No previous folder")
        } else if command == Forward && !self.can_forward {
            Some("No next folder")
        } else if command == Parent && !self.can_parent {
            Some("Already at the file system root")
        } else if matches!(command, Undo | Redo) {
            if self.history_busy || self.operations_active {
                Some("Wait for file operations to finish before undo or redo")
            } else if command == Undo && !self.can_undo {
                Some("There is nothing to undo")
            } else if command == Redo && !self.can_redo {
                Some("There is nothing to redo")
            } else {
                None
            }
        } else if mutation && self.history_busy {
            Some("Wait for undo or redo to finish before changing files")
        } else if command == Rename && self.items != 1 {
            Some("Select exactly one item to rename")
        } else if matches!(
            command,
            Copy | Cut
                | Move
                | CopyClipboard
                | Trash
                | DeletePermanent
                | SecureDelete
                | BatchRename
                | CreateArchive
        ) && self.items == 0
        {
            Some("Select one or more items first")
        } else if matches!(command, Copy | Move) {
            transfer(self.location, self.destination, command == Move).err()
        } else {
            None
        };
        decision
    }
}

impl AppModel {
    pub(super) fn location_policy(&self, path: &VPath) -> Location {
        Location::from_archive(
            self.archive_mounts
                .contains(path)
                .then(|| self.archive_mounts.read_only_reason(path).is_none()),
        )
    }
    pub(super) fn action_context(&self, pane: PaneId) -> Context {
        let path = self
            .folder_action_target
            .as_ref()
            .filter(|target| target.pane == pane)
            .map_or_else(
                || self.pane(pane).current_directory(),
                |target| &target.path,
            );
        let state = self.pane(pane);
        // Availability needs only the count, not a scan and allocation of every selected path.
        let items = if self
            .folder_action_target
            .as_ref()
            .is_some_and(|target| target.pane == pane)
        {
            1
        } else if !state.selection.is_empty() {
            state.selection.len()
        } else if state.view_mode == PaneViewMode::Columns {
            usize::from(state.miller_focus.is_some())
        } else {
            usize::from(
                state
                    .active()
                    .listing
                    .as_ref()
                    .and_then(|listing| listing.row(state.cursor_row as usize))
                    .is_some(),
            )
        };
        Context {
            location: self.location_policy(path),
            destination: self.location_policy(self.pane(pane.other()).current_directory()),
            items,
            history_busy: self.history_busy,
            operations_active: self.active_operations > 0,
            can_undo: !self.undo_stack.is_empty(),
            can_redo: !self.redo_stack.is_empty(),
            can_back: self.pane(pane).active().history_index > 0,
            can_forward: self.pane(pane).active().history_index + 1
                < self.pane(pane).active().history.len(),
            can_parent: self.archive_mounts.parent(path).is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_actions_cover_archive_access_selection_history_and_transfer_targets() {
        let regular = Context {
            items: 1,
            can_undo: true,
            ..Context::default()
        };
        let archive = Context {
            location: Location::Archive,
            ..regular
        };
        let read_only = Context {
            location: Location::ReadOnlyArchive,
            ..regular
        };
        for command in [
            CommandId::Rename,
            CommandId::Trash,
            CommandId::NewFile,
            CommandId::NewDirectory,
            CommandId::Paste,
        ] {
            assert!(archive.action(command).enabled());
            assert!(!read_only.action(command).enabled());
            assert!(!read_only.action(command).visible);
        }
        for command in [
            CommandId::Cut,
            CommandId::Move,
            CommandId::EditFile,
            CommandId::OpenWith,
            CommandId::Permissions,
            CommandId::BatchRename,
            CommandId::SecureDelete,
        ] {
            assert!(!archive.action(command).enabled());
            assert!(!archive.action(command).visible);
        }
        assert_eq!(
            archive.action(CommandId::Trash).label("Trash"),
            "Remove from archive…"
        );
        assert!(archive.action(CommandId::DeletePermanent).enabled());
        assert!(!archive.action(CommandId::DeletePermanent).visible);
        assert!(read_only.action(CommandId::CopyClipboard).enabled());
        assert!(read_only.action(CommandId::Copy).enabled());
        assert!(
            !Context {
                items: 0,
                ..regular
            }
            .action(CommandId::Copy)
            .enabled()
        );
        assert!(
            !Context {
                items: 2,
                ..regular
            }
            .action(CommandId::Rename)
            .enabled()
        );
        for command in [
            CommandId::Paste,
            CommandId::Trash,
            CommandId::Copy,
            CommandId::NewDirectory,
            CommandId::Undo,
        ] {
            assert!(
                !Context {
                    history_busy: true,
                    ..regular
                }
                .action(command)
                .enabled()
            );
        }
        assert!(
            !Context {
                destination: Location::ReadOnlyArchive,
                ..regular
            }
            .action(CommandId::Copy)
            .enabled()
        );
        assert!(
            Context {
                destination: Location::Archive,
                ..regular
            }
            .action(CommandId::Copy)
            .enabled()
        );
        assert!(
            !Context {
                destination: Location::Archive,
                ..regular
            }
            .action(CommandId::Move)
            .enabled()
        );
        assert!(regular.action(CommandId::Undo).enabled());
        assert!(!regular.action(CommandId::Redo).enabled());
        assert!(!regular.action(CommandId::Back).enabled());
        assert!(!regular.action(CommandId::Back).notify);
        assert!(!regular.action(CommandId::Forward).enabled());
        assert!(
            Context {
                can_back: true,
                can_forward: true,
                can_parent: true,
                ..regular
            }
            .action(CommandId::Back)
            .enabled()
        );
    }
}
