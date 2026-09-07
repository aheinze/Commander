# Carelo parity map

Reference: `aheinze/Carelo` at commit `174ef05`.

Commander reproduces Carelo as a native Rust application built with Relm4, GTK4,
and libadwaita. No Vue, WebView, JavaScript, or Tauri runtime is used. Directory and
metadata I/O stays behind the worker-backed VFS seam; long operations are cancellable
and report progress to the native work inspector.

## Implemented native surface

- Carelo-inspired light/dark shell, compact toolbar, resizable dual panes, sidebar,
  tabs, inspector, list/grid/Miller views, clickable breadcrumbs, and editable Ctrl+L
  address field. On displays narrower than 1440 px the inspector starts collapsed and
  remains available with Ctrl+I, preserving usable filename columns.
- Independent pane tabs, history, sort, hidden-file state, fuzzy filtering, type-ahead
  find with repeated-key cycling, range/multi-selection, and drag/drop moves.
- Image, text/code, audio, video, and interactive multi-page PDF previews with native
  paging, true scrollable zoom, and fit-to-page controls, plus a large Quick Look
  window with keyboard browsing.
- The resizable, persisted inspector mirrors Carelo's Info, Current Work, and Log
  surfaces: debounced previews, multi-selection summaries, recursive folder sizes,
  Git context, created/accessed metadata, R/W/X permissions with editing, retryable
  job cards, and structured status-aware activity entries.
- Copy/cut/paste, move, rename, four-mode batch rename, create, Trash/permanent delete,
  conflict resolution with apply-to-all and checksum comparison, parallel job cards,
  pause/resume/cancel, and undo/redo including exact Trash restoration.
- Recursive name/path/content search with regex, case, depth, type, extension, date,
  size, hidden, ignored/build-folder, symlink, and content-read-limit facets.
- SHA-256 calculation/comparison, folder compare and bidirectional sync with optional
  mirror-to-Trash, full Unix permission bits, and PolicyKit administrator retry.
- ZIP, 7z, TAR, and TAR.GZ create/extract plus read-only in-app archive browsing and
  copy-out; unsafe archive paths are rejected.
- Image conversion and native PDF compress/merge/extract/split/rotate/unlock tools.
- Finder-style persistent color tags in every view, pane tabs, and breadcrumbs.
- Embedded cancellable PTY terminal, system Open With/reveal, persistent favorites,
  recent locations, dual-pane workspaces, window state, and four color themes with
  system/light/dark appearance modes.
- Reorderable/removable favorites, named favorite groups, live desktop devices and
  mounts, and persistent remote endpoint shortcuts using the native GIO mount and
  credential flow.
- Searchable native right-click menus distinguish files, folders, multi-selections,
  and pane background space; they include Carelo's open/edit/reveal, new item,
  archive, checksum, PDF/image, transfer, permissions, tags, and delete actions.
- Persistent custom file/folder context-menu tools with safe argument parsing and
  path/name/parent placeholders, plus a functional parallel-transfer setting.
- Searchable keyboard-native command palette and an F1 shortcut reference generated
  from the active classic or modern keymap. The palette also indexes recent locations,
  favorites, workspaces, remotes, custom tools, sorting, inspector pages, and task
  controls; arrows, page keys, Home/End, Enter, and Escape work without a focus delay.

## Native implementation notes

- Archive browsing is materialized into a lifetime-scoped temporary directory and is
  guarded as read-only by the command layer. This preserves the native-path VFS model
  while allowing ordinary copy-out operations.
- Remote endpoints use the desktop GIO/GVfs providers. SFTP, FTP, WebDAV, and SMB work
  when their system backend is installed; S3 requires an installed GIO-compatible S3
  provider. Credentials are delegated to the desktop mount operation and are not saved
  in the session file.
- PDF page rendering uses the pure-Rust hayro/karet-pdf stack; editing uses lopdf.
- Elevated permission changes use the system PolicyKit prompt and never place a
  password in application memory or command-line arguments.

## Verification

Run the full gate before release:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p dualpane-app
```
