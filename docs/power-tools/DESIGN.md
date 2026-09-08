# Commander file-tool dialogs

This record covers Batch Rename, Find Duplicate Files, Compare Files, and Edit
Archive Contents. They extend the existing native utility dialogs and file panels;
they do not introduce a separate visual identity.

The implementation authority is `app/power_tools.rs`, `app/batch_rename_view.rs`,
`app/archive_editor.rs`, and the shared dialog rules in `assets/style.css`.

- Use `dialogs::utility_dialog` for the floating sheet, title, notification overlay,
  and shared main-window close control. Escape and the header close button retain
  the app’s native behavior. Cancel respects the same close permission during saves.
- Use the shared `dialog-body` inset and a separate `dialog-actions` footer. The
  footer is a sibling of the body, so its padding and separator are applied once.
- Toolbar actions use `dialog-button`, which shares the existing footer button
  styling: 34px minimum height, 4px corners, 12.5px semibold text, and a 90ms hover
  transition. Sentence-case verbs identify actions. Suggested actions use the
  existing settings contrast treatment across light, dark, and custom palettes.
- Form rows use 12.5px labels and the shared native entries, dropdowns, spin buttons,
  and checkboxes. Fields have accessible labels; text entries can activate the
  dialog’s default action where one is assigned. Archive saving has no implicit
  Enter default because adding a file and committing the reviewed plan are separate.
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
placement, validation, and action behavior. It captures all four dialogs at their
normal dark size and at 620px width in light mode, checking control bounds.
