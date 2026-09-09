//! Search actions carry their own targets; they never borrow a file pane's selection.
use super::*;

#[derive(Clone, Debug)]
pub(super) struct Session {
    pub pane: PaneId,
    pub root: VPath,
    pub options: SearchOptions,
}

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub command: CommandId,
    pub hits: Vec<SearchHit>,
    pub pane: PaneId,
    pub destination: VPath,
}

pub(super) fn supported(command: CommandId) -> bool {
    use CommandId::*;
    matches!(
        command,
        Open | OpenWith
            | Reveal
            | OpenInNewTab
            | OpenOtherPane
            | CopyClipboard
            | Cut
            | Paste
            | CopyPath
            | Copy
            | Move
            | Rename
            | BatchRename
            | Trash
            | DeletePermanent
    )
}

pub(super) fn selection_reason(command: CommandId, hits: &[SearchHit]) -> Option<&'static str> {
    use CommandId::*;
    if hits.is_empty() {
        return Some("Select one or more results first");
    }
    if matches!(
        command,
        Open | OpenWith | Reveal | OpenInNewTab | OpenOtherPane | Rename | Paste
    ) && hits.len() != 1
    {
        return Some("Select exactly one result");
    }
    if command == Paste && !hits[0].kind.is_directory() {
        return Some("Select a folder to paste into");
    }
    if command == OpenWith && hits[0].kind.is_directory() {
        return Some("Select a file to open with another application");
    }
    if command == OpenInNewTab && !hits[0].kind.is_directory() && !is_archive_path(&hits[0].path) {
        return Some("Select a folder or archive to open in a new tab");
    }
    None
}

/// A selected folder already includes its descendants. Keep links independent:
/// a selected directory symlink must never swallow results from its target.
pub(super) fn operation_paths(hits: &[SearchHit]) -> Vec<VPath> {
    let directories: BTreeSet<_> = hits
        .iter()
        .filter(|hit| hit.kind.is_directory())
        .map(|hit| hit.path.as_path())
        .collect();
    hits.iter()
        .filter(|hit| {
            !hit.path
                .as_path()
                .ancestors()
                .skip(1)
                .any(|p| directories.contains(p))
        })
        .map(|hit| hit.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

impl AppModel {
    pub(super) fn search_action(
        &mut self,
        request: Request,
        confirmed: bool,
        sender: &ComponentSender<Self>,
    ) {
        let Request {
            command,
            hits,
            pane,
            destination,
        } = &request;
        if !supported(*command) {
            return;
        }
        if let Some(reason) = selection_reason(*command, hits) {
            notifications::error(reason);
            return;
        }
        // Recheck policy at dispatch (and again after confirmation), since a
        // menu may have opened before undo or an archive access change.
        for hit in hits {
            let context = action_policy::Context {
                location: self.location_policy(&hit.path),
                destination: self.location_policy(destination),
                items: hits.len(),
                ..self.action_context(*pane)
            };
            if let Some(reason) = context.action(*command).reason {
                notifications::error(reason);
                return;
            }
        }
        let first = &hits[0];
        match command {
            CommandId::Open => {
                if first.kind.is_directory()
                    || (self.workflow.browse_archives && is_archive_path(&first.path))
                {
                    self.on_close_search();
                }
                self.open_path(*pane, first.kind, first.path.clone(), sender);
            }
            CommandId::Reveal => {
                self.on_close_search();
                self.reveal_path(*pane, first.path.clone(), sender);
            }
            CommandId::OpenWith => show_open_with_dialog(first.path.clone()),
            CommandId::OpenOtherPane => {
                self.on_close_search();
                if first.kind.is_directory() || is_archive_path(&first.path) {
                    self.open_path(pane.other(), first.kind, first.path.clone(), sender);
                } else {
                    self.reveal_path(pane.other(), first.path.clone(), sender);
                }
            }
            CommandId::OpenInNewTab => {
                self.on_close_search();
                let state = self.pane_mut(*pane);
                state.remember_navigation();
                state.tabs.push(TabState::new(first.path.clone()));
                state.active_tab = state.tabs.len() - 1;
                state.reset_for_folder_entry();
                state.tabs_revision = state.tabs_revision.wrapping_add(1);
                self.start_listing(*pane, sender);
                self.persist_session();
            }
            CommandId::CopyClipboard | CommandId::Cut => {
                self.copy_paths_to_clipboard(&operation_paths(hits), *command == CommandId::Cut);
            }
            CommandId::Paste => self.paste_file_clipboard_into(*pane, first.path.clone(), sender),
            CommandId::CopyPath => {
                if let Some(display) = gdk::Display::default() {
                    display.clipboard().set_text(
                        &hits
                            .iter()
                            .map(|hit| self.archive_mounts.display(&hit.path).to_string())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
            }
            CommandId::Rename => show_rename_dialog(first.path.clone(), sender),
            CommandId::BatchRename => {
                // Renaming a folder and a child in the same batch invalidates
                // the child's source. Use the same non-overlapping target set.
                show_batch_rename_dialog(operation_paths(hits), sender);
            }
            CommandId::Trash | CommandId::DeletePermanent
                if hits
                    .iter()
                    .any(|hit| self.is_archive_browse_path(&hit.path)) =>
            {
                self.review_archive_removal_on_paths(*pane, operation_paths(hits), sender);
            }
            CommandId::DeletePermanent if !confirmed => {
                let Some(window) = relm4::main_application().active_window() else {
                    return;
                };
                let paths = operation_paths(hits);
                let names = paths
                    .iter()
                    .take(5)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                let body = format!(
                    "Permanently delete {} selected {}? This action cannot be undone.\n\n{names}{}",
                    paths.len(),
                    if paths.len() == 1 { "item" } else { "items" },
                    if paths.len() > 5 { "\n…" } else { "" },
                );
                let dialog = AlertSheet::new(Some("Delete Permanently?"), Some(&body));
                dialog.add_response("cancel", "Cancel");
                dialog.add_response("delete", "Delete");
                dialog.set_close_response("cancel");
                dialog.set_default_response(Some("cancel"));
                dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                let input = sender.input_sender().clone();
                dialog.connect_response(Some("delete"), move |_, _| {
                    let _ = input.send(AppMsg::SearchDeleteConfirmed(request.clone()));
                });
                dialog.present(Some(&window));
            }
            CommandId::Copy | CommandId::Move | CommandId::Trash | CommandId::DeletePermanent => {
                self.start_operation_on_paths(
                    *command,
                    *pane,
                    operation_paths(hits),
                    destination.clone(),
                    sender,
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, kind: EntryKind) -> SearchHit {
        SearchHit {
            path: VPath::from(path),
            kind,
            size: 1,
            content_match: false,
        }
    }

    #[test]
    fn search_operation_targets_cover_children_without_following_links() {
        let hits = vec![
            hit("/one/folder/child", EntryKind::File),
            hit("/one/folder", EntryKind::Directory),
            hit("/one/folder-extra", EntryKind::File),
            hit("/two/another", EntryKind::File),
            hit("/one/link", EntryKind::Symlink),
            hit("/one/link/child", EntryKind::File),
            hit("/two/another", EntryKind::File),
        ];
        assert_eq!(
            operation_paths(&hits),
            [
                "/one/folder",
                "/one/folder-extra",
                "/one/link",
                "/one/link/child",
                "/two/another"
            ]
            .map(VPath::from)
        );
        assert!(selection_reason(CommandId::Rename, &hits).is_some());
        assert!(selection_reason(CommandId::Trash, &hits).is_none());
        assert!(selection_reason(CommandId::Trash, &[]).is_some());
    }
}
