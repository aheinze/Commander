# Commander file-tool dialogs

This record covers Batch Rename, Find Duplicate Files, Compare Files, and archive
operations in the file panels. They extend the existing native utility dialogs and file panels;
they do not introduce a separate visual identity.

The implementation authority is `app/power_tools.rs`, `app/batch_rename_view.rs`,
`app/archive_actions.rs`, and the shared dialog rules in `assets/style.css`.

- Use `dialogs::utility_dialog` for the floating sheet, title, notification overlay,
  and shared main-window close control. Escape and the header close button retain
  the app’s native behavior.
- Use the shared `dialog-body` inset and a separate `dialog-actions` footer. The
  footer is a sibling of the body, so its padding and separator are applied once.
- Toolbar actions use `dialog-button`, which shares the existing footer button
  styling: 34px minimum height, 4px corners, 12.5px semibold text, and a 90ms hover
  transition. Sentence-case verbs identify actions. Suggested actions use the
  existing settings contrast treatment across light, dark, and custom palettes.
- Form rows use 12.5px labels and the shared native entries, dropdowns, spin buttons,
  and checkboxes. Fields have accessible labels; text entries can activate the
  dialog’s default action where one is assigned.
- Tables use the panel’s foreground, zebra, hover, selection, and focus tokens.
  Numeric columns remain narrow and paths receive the spare width. Column dividers
  are omitted. Comparison contents use monospace; other copy uses the app font.
- Counts, progress, instructions, and durable recovery paths stay in the content
  area as 12px muted text. Transient errors and completion notices use the existing
  `notifications::Feedback` toast surface. Validation also marks the relevant field
  and retains its diagnostic in the accessible description and tooltip.
- Stop is visible while work is running. Controls stay available at compact widths;
  labels wrap and file paths truncate with full-path tooltips.

The existing native power-tool workflow test checks the shared shell, footer
placement, validation, and action behavior. It captures the three tool dialogs at their
normal dark size and at 620px width in light mode, checking control bounds.

Archive operations use the existing panel menus, rename/create sheets, conflict
sheet, and Jobs controls. There is no separate archive editor. Removal uses the
shared alert sheet and main-window close control, with Cancel as the default and
a destructive Remove action. Writable archive menus say “Remove from archive…”;
read-only archives omit mutations. Unsupported actions are omitted in either case.
The archive panel workflow test covers all three views, two panes and inactive
tabs, import conflicts, captured removal targets, and recovery copies. It also
captures the conflict and removal sheets in dark and light appearances.

Action availability and contextual labels live in `app/action_policy.rs`. Menus,
command dispatch, the toolbar, palette, and transfer/drop checks consume that
policy; backend checks still validate the actual files before mutation. Use the
same contextual wording for archive removal and the same disabled reason across
surfaces. Tab menus use their clicked folder's location and current operation state.

Archive undo, redo, and manual recovery share the verified archive transaction.
Recovery uses the existing operation-details alert with a destructive “Restore
archive” action and the shared window close control. Its copy explains which
version will be restored, the unchanged-file requirement, and that the restore
can be undone. Recovery paths remain available in the operation details.

Archive creation and unlock sheets use `utility_dialog`, the shared window close
control, native password entries with a reveal control, and the standard footer.
Creation requires matching nonempty passwords when protection is enabled. ZIP and
7Z explain whether names are visible; TAR formats disable protection explicitly.
Unlock requests are tied to the original worker and pane generation. Cancellation
closes the sheet and cannot navigate a different tab after a late password reply.
