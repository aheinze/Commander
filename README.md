# Commander

Commander is a native, keyboard-first, dual-pane file manager written in Rust. Its
primary target is Linux on Wayland with GNOME, GTK4, libadwaita, and Relm4.

The project is being delivered in reviewable milestones. M3 adds stable multi-selection,
cancellable type-ahead filtering, classic and modern keymaps, a searchable command
palette, glob selection, bookmarks, and a collapsible GVfs-aware sidebar to the M2
native shell. The GTK-free indexing pipeline remains the source of cancellable immutable
directory snapshots and filtered row-order views.

## Workspace

- `dualpane-core`: shared domain and cancellation primitives
- `dualpane-vfs`: virtual filesystem seam and local implementation
- `dualpane-engine`: cancellable file-operation jobs
- `dualpane-index`: directory snapshots, sorting, filtering, and watch diffing
- `dualpane-thumbs`: thumbnail scheduling and caches
- `dualpane-platform`: the sole home for platform-specific code and audited `unsafe`
- `dualpane-app`: Relm4 application and binary
- `xtask`: repository automation and fixture generation

The domain, VFS, engine, and index crates remain independent of GTK and can be built
and tested without a display server.

## Developer commands

```console
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask gen-fixtures
cargo xtask check-m1-budgets
cargo run --release --package dualpane-app
```

Run `python3 scripts/test-native.py --backend wayland` for isolated native interaction
regressions, including the tests normally ignored by `cargo test`. Logs and screenshots
are saved under `target/native-tests/`. See [release checks](docs/RELEASE_CHECKS.md) for
dependencies, performance gates, and the installation/remote checks needed before release.

For everyday development, `./dev.sh` launches the current working tree using the
fast debug profile. Use `./dev.sh --release` for realistic performance, or place
Commander arguments after `--`, such as `./dev.sh -- --left /tmp`.

`cargo xtask gen-fixtures` creates `target/fixtures/large-tree` atomically. Pass
`--root`, `--files`, `--files-per-directory`, or `--payload-bytes` to override its
deterministic defaults. It refuses to overwrite an existing path.

The M1 latency gate expects one flat 100k-entry directory. Generate it and run the
release-mode checks with:

```console
cargo xtask gen-fixtures --root target/fixtures/flat-100k --files 100000 --files-per-directory 100000
cargo xtask check-m1-budgets --root target/fixtures/flat-100k/bucket-000000
cargo bench --package dualpane-benches --bench index_pipeline
```

## Appearance

The interface uses flat surfaces, compact controls, and a shared type scale across
file panes, the sidebar, menus, dialogs, and the inspector. All text in the file
panes uses monospace, including names, metadata, headers, paths, tabs, and status
labels in List, Grid, and Column views. Source previews and terminals also use
monospace text. File and terminal tabs use compact rounded pills, fine outlines,
and a soft active fill. Selections remain distinct from keyboard focus, and item
counts appear once in each pane's status bar.

Light and dark appearances share the same layout. Automatic color themes continue
to follow the desktop palette; the built-in palette uses neutral surfaces and a
restrained blue accent.

List and Column views use faint alternating row backgrounds that follow the text
palette in light and dark themes. Hover, selection, and keyboard focus take
precedence; Grid keeps a uniform background. Striping uses GTK CSS only.

Controls use bundled Lucide outline icons. List, Grid, Columns, search, and preview
fallbacks share distinct folder, source, document, image, audio/video, archive,
spreadsheet, and other file-type symbols with muted theme-aware accents. Icons are
embedded in the executable and cached by GTK; file classification uses names only,
with no additional metadata reads. The bundled licenses and pinned source are in
`crates/app/assets/icons/`. Building the app requires `glib-compile-resources`
from the GLib development tools alongside the GTK/libadwaita development packages.

Terminal tabs retain visible close buttons. New and selected tabs scroll into
view; arrows and mouse-wheel scrolling keep additional tabs reachable. Tabs with
the same folder name show a session
number, and tooltips include their starting folder. The terminal options menu
contains Close all terminals; hiding the panel keeps its sessions running.

## Toolbar navigation

The top bar provides Back, Forward, Parent folder, and Refresh controls. Back and
Forward are disabled when no history is available. The folder title follows the
active pane, including the last Miller column; click it to edit the location.

Drag the empty space beside the folder title or the sidebar's window controls to
move the window. These areas also support the desktop's standard titlebar gestures,
including double-click to maximize or restore and right-click for the window menu.

New groups folder, file, archive, and tab creation. The current view menu selects
List, Grid, or Columns and toggles hidden files for the active pane. Layout controls
the locations sidebar, second file pane, inspector, terminal, and panel orientation.
More contains search, the command palette, file operations, Recovery, and Settings.
Menus and tooltips display the shortcuts from the current keymap.

File context menus put opening, copying, renaming, and Trash first. **More actions**
contains specialist tools, archives, tags, and permanent deletion. Start typing to
search every available action, including those under More actions. Arrow keys move
between actions, Enter runs the focused action or first search result, and Escape
closes the menu without clearing the file selection. Menus adapt to the clicked
item or selection; **Paste into folder** uses the clicked folder as its destination.

In Settings, **Manage Context Menu Tools…** opens a list of custom actions with
enable switches, editing, and removal with Undo. Add or edit an action using named
fields, a placeholder picker, a command preview, and file-type rules. Saved actions
and list changes apply immediately; unfinished edits can be cancelled.

The name filter stays centered in the top bar, with a capped width on large windows
and its own centered row at narrow widths. Resizing preserves the query and text
selection. It names the pane it filters and highlights when a filter is applied.
Escape clears it and returns focus to the files. At compact widths, menu labels
collapse to icons while all actions remain available in their labeled menus.

Breadcrumbs open the exact folder you click, including in Columns. Clicking the
current folder keeps its selection and history intact. The parent-folder menu
keeps every ancestor reachable on deep paths and narrow panes. Location edits
survive background updates; Escape cancels the edit without clearing file selection.

Right-click a Favorite and choose **Rename…** to give the shortcut a custom label.
This also works inside favorite groups. Clearing the label restores the folder name;
the shortcut continues to open its original location.

The sidebar updates automatically when devices connect, mount, or disconnect.
Use the eject button beside a device to safely remove it, or unmount it when
hardware ejection is unavailable. Removing a drive includes its other mounted
volumes; the button's tooltip identifies this. Busy devices stay connected and
show a retry message. Tabs on a removed device return to Home.

## Column browsing

In Column (Miller) view, drag a column's right divider to resize it, or double-click
the divider to reset its width. Folder headers show item counts; ancestor selections
stay visible as you browse deeper. Single-click a file to preview it and double-click
or press Enter to open it. Ctrl/Shift-click supports multiple selections, and failed
folder loads include a Try Again action. The last column is the current folder for
paste, creation, cross-pane transfers, search, and terminals. The location bar shows
that folder; drops on a column target that column's folder.

Hovering over ancestor rows leaves the open columns and selection unchanged.
A single click opens a folder; clicking an already-open ancestor keeps its nested
branch in place.

Refresh reloads the whole visible branch, retaining valid selections by name. Sorting
and hidden-file changes update the columns; filtering narrows the last column while
keeping ancestor folders visible. Missing branches are removed after refresh.
Column widths, expanded paths, selections, and scroll offsets survive restarts and
navigation between tabs/folders. Sort and hidden-file preferences are restored when
returning to a folder. The chosen List, Grid, or Columns mode stays with the pane
across directory and tab navigation and is saved for the next launch.

## Live folder updates

List, Grid, and every open Miller column update when files are created, edited,
renamed, or deleted by another application. The focused preview and Quick Look
reload after external edits. Updates retain surviving selections and scroll positions.

Native notifications are shared when panes show the same folder and batched for
200 ms. Snapshot updates, metadata reads, sorting, and filtering run on background
workers; ordinary changes read metadata only for the affected names. Content-only
updates reuse row storage and refresh only changed visible rows. Each pane has at
most one update worker, and event overflow requests a single fresh listing.

Only visible folders are monitored; reopening a hidden pane or tab refreshes its
contents. If native monitoring cannot be established, that folder is checked every
five seconds while visible, with monitoring retried. Idle native watches do not poll.

## Text previews

The inspector and Quick Look highlight source files using bundled grammars and a
native Rust parser, with colors matched to the current light or dark appearance.
Supported formats include Rust, C/C++, Python, Ruby, Go, Java, JavaScript (including
JSX, MJS, and CJS), PHP/PHTML, TypeScript (including TSX, MTS, and CTS), Vue, Svelte,
HTML/CSS, JSON, YAML, TOML, INI, SQL, and shell scripts, plus other formats in the
bundled language registry. No external highlighting program is needed.

Detection also recognizes Dockerfile (including Dockerfile.dev), Makefile,
CMakeLists.txt, Cargo.lock, PKGBUILD, .env files, .gitignore, and common shell dotfiles.
TXT, logs, CSV, generic .conf files, and extensionless README/license files remain
plain text. Text previews read at most 512 KiB, reject NUL-containing binary samples,
and limit highlighting work; any uncolored remainder stays readable.

## File-operation guarantees and limits

Copy, move, trash, and delete jobs appear in a slim, single-row activity strip below
the file panes, even when the inspector is hidden. It shows a small progress bar,
status, and pause/resume/cancel controls. Transfer speed, time remaining, file paths,
and errors are available in the status tooltip and Jobs menu. Finished jobs keep a
quiet status label without a filled progress bar. Open Jobs for individual operations;
the latest 20 completed, cancelled, or failed jobs remain available until dismissed.
Failed and cancelled jobs can be retried there. The inspector's Work tab shows the
same details. Progress reaching 100% can be followed by a finishing phase while the
engine verifies and synchronizes writes. Copying, verification, and finishing have
distinct labels. Pause and cancellation are checked between range-copy chunks and
verification reads; a kernel I/O call already in progress must return first. The job
details show the destination as well as the current file.

Archive creation and extraction also appear in the activity strip and Jobs list,
including when the inspector is hidden. A small spinner indicates work when the total
is unknown. Processed bytes/items, the current file, and the destination appear in
the tooltip and Jobs list; pause and cancel are checked between reads.
Creation stays visible while the archive is finalized and synchronized. Completed,
cancelled, and failed archive jobs remain available for inspection. New archives are
published only after successful completion; cancellation removes their temporary output.
Extraction can leave already extracted files in the destination if interrupted.
Archives are saved beside the selected items. In Column view, opening a folder keeps
that folder selected for archiving until an item in another column is selected.
Archive creation rejects destinations inside a source folder to avoid including its
own temporary output.

Copies started in the app verify regular-file contents before publishing and
synchronize writes. Cross-filesystem moves use the same verified copy path. Moving into the same directory is a no-op; moving or copying
a directory into itself (including through a symbolic-link alias) is rejected.
Skipped items and source files changed during a move are retained. Directory cleanup
removes only scanned, successfully copied entries and never recursively deletes
unscanned or newly created children.

Keep Both selects a distinct name for every conflict. Batch rename, undo, and redo
stage overlapping names and refuse unexpected destination collisions. Failed
operations are reported as failed and remain available for inspection/retry.

Undo uses the actual transfer outputs, including Keep Both, overwrites, and directory
merges. Original overwritten items are retained beside their destinations under hidden
`.commander-undo-…` names. Undo checks identity, size, and modification times and refuses
outputs with unexpected edits or new children. Copy redo restores the copied data from
Trash, even if its source changed. Undo/redo history is saved across restarts; emptying
Trash or removing retained backups can make a history entry unavailable.

More → Recovery and saved operations opens durable operation records. A journal is synchronized
before publication, source removal, and other transfer mutations. If it cannot be
created, the operation fails before changing files. Interrupted jobs are surfaced on
startup; live jobs owned by another Commander instance are excluded. Recovery shows
completed destinations, uncertain steps, and retained originals. **Restore missing
originals** fills only absent paths and refuses changed backups. Existing destinations
are never overwritten by that recovery action. Retained originals and temporary outputs
are not automatically deleted. Inspect the listed locations before removing them.
An interrupted undo/redo is archived for review and its history is not automatically
replayed. Records live in the application's XDG state directory (`dualpane/jobs` and
`dualpane/history.json`).

File copy/cut/paste uses the desktop clipboard, including URI lists and GNOME/KDE cut
markers. Local files from other applications can be pasted into the visible folder.
The destination is captured before reading clipboard data, so navigation during an
asynchronous read cannot redirect the paste. Unavailable remote URI lists are rejected
as a whole; connect the remote location before copying its files.

Operations are not one transaction across an entire selection. Cancellation or a
failure can leave completed items in place. Batch-rename rollback reports any files
it could not restore, including their recovery paths; it never overwrites a new
occupant while recovering. A process or machine crash during a directory/type
replacement or batch rename can leave recovery items containing `.dualpane-tmp-`
or `.omacommander-rename-` in their names. Keep those items until their contents
have been inspected. These checks do not provide a
filesystem-wide lock against other programs changing files at the same time.

Regression coverage includes skipped moves, renamed destinations, conditional
conflicts, overlapping renames, rollback failures, directory aliases, cancellation,
and undo/redo preservation. Run `cargo test --workspace --locked` and
`cargo clippy --workspace --all-targets --locked -- -D warnings` before release.

## Native application

Commander needs GTK 4.14+ and libadwaita 1.5+ development packages. On Arch/Omarchy,
install `gtk4` and `libadwaita`; on Ubuntu 24.04, install `libgtk-4-dev` and
`libadwaita-1-dev`.

Launch two chosen directories with:

```console
cargo run --release --package dualpane-app -- --left /path/one --right /path/two
```

The shell is keyboard-operable: `Tab` switches panes, arrows move the cursor in the
current view, and `Home`/`End`/`Page Up`/`Page Down` handle longer listings. `Insert` or
`Shift+Space` toggles selection, `Ctrl+A` selects all, `Ctrl+Shift+A` clears, `*`
inverts, and `+`/`-` open glob selection. Printable typing starts fuzzy filtering;
`Esc` clears the topmost panel, then the filter, then selection. `Ctrl+Shift+P`, the
Menu key, or `Shift+F10` opens the command palette; it supports type-to-filter,
arrow/page navigation, and Enter-to-run without an input focus delay. `F9` toggles the
sidebar, `Ctrl+Shift+B` focuses it, `Ctrl+Alt+I` focuses the inspector, and `Ctrl+F6`
returns focus to the active file view. State is written atomically by a background
worker.

Classic bindings are the default. Switch to the modern profile from the command
palette. The palette also exposes favorites, recent paths, workspaces, remotes, custom
tools, sorting, inspector pages, and background-task controls. Both bundled profiles
live in `crates/app/assets/keymaps.toml`; override only
the commands you want in `$XDG_CONFIG_HOME/dualpane/keymaps.toml`, for example:

```toml
[classic]
switch-pane = ["<Control>Tab"]

[modern]
refresh = ["F5"]
```

Profile both panes against the flat 100k fixture with:

```console
target/release/commander --benchmark target/fixtures/flat-100k/bucket-000000
target/release/commander --benchmark target/fixtures/flat-100k/bucket-000000 \
  --benchmark-filter entry-09999
```

The benchmark prints `DUALPANE_STARTUP` and `DUALPANE_SCROLL` records containing
paint cadence, dropped-frame count, maximum bind/render/scroll callback time,
realized-cell count, and RSS. Keep the window visible on Wayland; compositors throttle
frame clocks for obscured windows. Add `--quit-after-first-paint` for a startup-only
smoke test, or `--profile-startup` to enable tracing without automatic scrolling.
The filter variant waits for the active pane's reusable matcher to be warm, then prints
a `DUALPANE_FILTER` response/worker/queue breakdown and exits.
