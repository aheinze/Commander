# Commander reference

Detailed usage, implementation notes, file-operation behavior, and development
commands. For a product overview and quick start, see the [README](../README.md).
Commands and file paths below assume the repository root.

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
are saved under `target/native-tests/`. See [release checks](RELEASE_CHECKS.md) for
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

## Settings and keyboard customization

Open Settings from the command palette or the main menu. Workflow controls restore
previous tabs on startup, keep folders first, browse archives inside Commander, and
choose parallel or sequential transfers. Context menu tools are available from the
same page.

The Keyboard page lets you switch between Classic and Modern profiles, search
commands, and record a replacement shortcut. A conflict names the action currently
using the key; Assign moves that shortcut while preserving its other bindings.
Remove leaves an action unassigned. Reset restores defaults for the selected profile.
Tab navigates the recorder, Enter assigns a captured key, and Escape cancels.
Custom bindings apply in the file panes; text fields, dialogs, and terminals retain
their own navigation. Overrides are saved to the existing `keymaps.toml` file.

Appearance includes color themes and switches for the inspector's folder-size
scans and Git information. History controls recent-location recording and its
limit, with a clear action. This only affects the Recent list; tabs, per-folder
state, bookmarks, workspaces, and operation recovery are separate. Disabling tab
restoration starts in Home on the next launch; explicit `--left` and `--right`
paths take precedence.

Settings changes are staged until Apply. Cancel or closing the dialog discards them.
About shows the app version, creator profile, license, platform and runtime versions,
source/issue links, and a Copy details button that excludes paths and personal data.
**Check for updates** checks stable GitHub Releases on request. Newer releases show
release notes and compatible package downloads when the release signature can be
verified. Downloads require a local save location and are published only after
their signed size and SHA-256 match. Installation remains manual.

Update checks, downloads, and **Skip this version** take effect immediately and
are separate from Apply/Cancel. Closing Settings cancels unfinished update work.
The last successful check, API cache, rate-limit backoff, and skipped version are
stored in `updates.json` beside the session state. A manual check can revisit a
skipped version. There are no automatic network checks or automatic installations.
See the [packaging guide](../packaging/README.md#in-app-update-behavior) for signing
setup, compatibility, cancellation, and temporary-file limits.

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

Each file panel's status bar has a terminal button on the right. It opens a new
terminal tab in that panel's current folder, reveals the terminal panel, and
focuses the new session.

In Git working trees, each panel's status bar also shows a muted branch indicator
for its current folder, following tab changes and the last column. `*` marks
uncommitted changes, or `!` marks merge conflicts; nonzero `↑` and `↓` counts show
commits ahead of and behind the locally known upstream, without fetching. Hover
for the full branch name, working-tree status, and tracking details. The indicator
stays hidden outside repositories, when Git is unavailable, or when a query is
unsupported or fails.

Terminal tabs retain visible close buttons. New and selected tabs scroll into
view; arrows and mouse-wheel scrolling keep additional tabs reachable. Tabs with
the same folder name show a session
number, and tooltips include their starting folder. The terminal options menu
contains Close all terminals; hiding the panel keeps its sessions running.

## Notifications

Errors, information, and completion feedback appear as dismissible toasts above
the main window or active dialog. Errors stay for 10 seconds; information and
success for 5 seconds. Action toasts stay for 10 seconds. **Copy** preserves the
full diagnostic, **Undo** restores a removed custom action, and **Options** opens
remote connection recovery choices. Long text wraps and has a full-message tooltip.

The shared queue retains at most four toasts and protects errors from information
traffic. Repeated messages and unchanged model state do not flood the screen.
Field validation waits briefly while typing; its tooltip and accessible description
retain the current error after the toast closes. Counts, progress, metadata, static
instructions, durable reports, and safety confirmations remain in the interface.
See the [notification design](notifications/DESIGN.md) for behavior and appearance
rules; the implementation lives in `crates/app/src/app/notifications.rs`.

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

Sidebar locations support right-click, the Menu key, and Shift+F10. Places,
favorites (including grouped favorites), recent folders, devices, connected
mounts, saved remotes, workspaces, favorite groups, and Trash use the same menu
surface, icon rows, focus styling, and separators as file menus. Arrow keys move
between available actions; Escape dismisses the menu.

Click a sidebar group heading to collapse or expand its contents. This works for
Places, Favorites, Devices, Remote Storage, Workspaces, Recent, and custom favorite
groups. Space or Enter toggles the focused heading; Left collapses and Right
expands it. Commander remembers these choices between launches. Header add buttons
remain available when collapsed and reveal the section when used; Trash stays
pinned below the groups.

Folder menus open the clicked location in the active pane, a new tab, the other
pane, or a new terminal, and copy its path. Favorites retain their rename,
reorder, and removal controls; groups offer adding the current folder and group
removal. Recent menus can add a favorite, remove an entry, or clear the list.
Device menus show mount and safe removal actions when supported. Saved remotes
offer connect, edit, forget, and copy address; mounted remotes offer disconnect.
Workspace menus open, update, rename, or remove the saved setup. Removing a
sidebar shortcut or history entry does not delete the folder it points to.

The toolbar, file and tab menus, command palette, keyboard commands, and file
drops share action availability rules. Archive actions use the same wording at
each entry point, including **Remove from archive…**. Unsupported archive commands
are omitted from context menus and the palette; disabled controls provide an
explanation in their tooltip. File changes remain unavailable while undo, redo,
or archive recovery is running.

File context menus put opening, copying, renaming, and Trash first. **More actions**
contains specialist tools, archives, tags, permanent deletion, and **Secure delete…**. Start typing to
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

Opening a folder normally or in a new tab starts with no selected files, even if
files were selected on a previous visit. Back and Forward restore the selection
from the folder you left. Search results still select the specific file being revealed.

Breadcrumbs open the exact folder you click, including in Columns. Clicking the
current folder keeps its selection and history intact. The parent-folder menu
keeps every ancestor reachable on deep paths and narrow panes. Location edits
survive background updates; Escape cancels the edit without clearing file selection.

Right-click a Favorite and choose **Rename…** to give the shortcut a custom label.
This also works inside favorite groups. Clearing the label restores the folder name;
the shortcut continues to open its original location.

Right-click a saved Remote Storage entry, or press Shift+F10 while it is focused, for
**Connect**, **Edit connection…**,
and **Forget connection**. Set an optional **Name** when creating or editing a
connection to label it in the sidebar, recent connections, and command palette.
The full address remains in the sidebar tooltip. Clearing Name restores the address
label; changing the address keeps the name. The editor restores the saved name,
protocol, server, port, folder, and username. **Save** updates the entry without
connecting; **Save and connect** also connects and can use a new password. Existing keyring passwords are
kept, and passwords never appear in the saved address. **Cancel** leaves the entry
unchanged.

A saved connection and its active mount share one sidebar row. The row keeps the
saved name, opens the saved folder, and stays highlighted within its subfolders.
Connected entries offer folder actions and **Disconnect**. Disconnecting keeps the
saved connection; **Forget connection** removes the shortcut while leaving the
mount connected. Other accounts, ports, and shares remain separate.

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

## PDF previews

The inspector and Quick Look display PDFs as a continuously scrolling document.
Fit-to-width follows the available space. Use the page field or previous/next
buttons to jump, and the bookmarks button to open the document's table of contents
when one is present. Zoom keeps your vertical reading position.

When the PDF is focused, use Ctrl+L to enter a page number, Ctrl++/Ctrl+− to zoom,
Ctrl+0 to fit the width, Alt+↑/Alt+↓ for adjacent pages, and Home/End for the first
or last page. Arrow keys and Page Up/Page Down scroll the document. Click the file
panel or use the configured Focus files shortcut to return to file navigation.

Rendering stays on a background worker with nearby pages cached and offscreen
page widgets released. PDF previews accept files up to 128 MiB and 10,000 pages;
each page raster is limited to four million pixels and a 4096-pixel edge. Text
selection, text search, OCR, and password-protected PDF previews are not supported.

## Spreadsheet previews

Excel (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`), CSV, and TSV files open as read-only
tables in the inspector and Quick Look. Scroll in either direction, resize
columns, select and copy cell text, and choose a worksheet in Excel workbooks.
Column letters and row numbers preserve spreadsheet coordinates. CSV's first row
remains visible as data, so files without headers do not lose a record.

CSV detects commas, semicolons, tabs, and pipes and supports quoted fields,
multiline values, UTF-8, and UTF-16 with a byte-order mark. Excel displays cell
values, dates, and saved formula results; formulas and macros are not executed.
Workbook formatting, charts, and merged-cell layouts are not reproduced.

Preview parsing runs on a worker. Each table shows up to 500 rows and 50 columns,
with up to 20 worksheets, 2 KiB per cell, and 4 MiB of retained cell text. A notice
identifies limited previews. CSV reads up to 4 MiB; Excel accepts up to 32 MiB with
bounded archive expansion. Legacy XLS files also have a one-million-cell combined
worksheet-area limit. Empty, unreadable, encrypted, or oversized files show a
preview message. External file changes refresh the preview.

## SVG previews

SVG files render as images in the inspector and Quick Look, with transparent
backgrounds, system fonts, and sharp scaling up to a 1600-pixel preview edge.
Rendering runs on the preview worker and accepts SVG text up to 8 MiB. Embedded
raster images are bounded; referenced local/remote images and nested SVG images
are omitted. Invalid SVGs show a preview error. Other source files retain their
text previews.

## Text previews

Markdown files (`.md`, `.markdown`, `.mdown`, `.mkd`, and `.mkdn`) render as formatted
documents in the inspector and Quick Look. Headings, emphasis, nested lists, task
lists, quotations, fenced code, tables, and footnotes use native GTK widgets.
Text and code remain selectable. Heading links scroll within the preview; relative
file links resolve beside the document, and supported external links open on click.
The layout and colors follow the current appearance.

Local Markdown images load on the preview worker. Up to eight local image references
are attempted, with a 2 MiB encoded-size limit per image and bounded decoding. Remote,
missing, or oversized images show their descriptions. Previewing does not fetch
remote resources, and embedded HTML is displayed literally. Markdown shares the
512 KiB text limit and also bounds structure, nesting, and widget count; a notice
appears when the document is truncated.

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
Eligible failed and cancelled jobs can be retried there. The inspector's Work tab shows the
same details. Progress reaching 100% can be followed by a finishing phase while the
engine verifies and synchronizes writes. Copying, verification, and finishing have
distinct labels. Pause and cancellation are checked between range-copy chunks and
verification reads; a kernel I/O call already in progress must return first. The job
details show the destination as well as the current file.

ZIP, 7Z, TAR, TAR.GZ, and TGZ archives can be browsed like folders in List, Grid,
and Columns. The Workflow setting for archive browsing controls the default Open
action; **Browse archive** in the context menu works regardless of that setting.
Breadcrumbs and tabs show archive names and icons, with Parent, Back, and Forward
navigation, nested archives, opening in a new tab or the other pane, and restoration
of saved archive locations. Opening extracts the archive to temporary storage in
the background; each pane shows a loading state with Escape to cancel. The footer
identifies archive locations and marks read-only archives explicitly. Copy, paste,
or drag files and folders into writable archives; rename, create folders or empty
files, and remove entries directly in the panels. See **Editing archives** below.

**Create Archive → Protect with a password** encrypts ZIP file contents with AES-256.
7Z uses AES-256 for both contents and file names; ZIP names remain visible.
TAR and TAR.GZ do not support passwords. Enter and confirm the password before
creating a protected archive. Opening or extracting an encrypted ZIP or 7Z prompts
for its password, with retry and cancellation. Existing ZipCrypto ZIPs can also be
opened. Encrypted streams are verified before extraction writes destination files.
Passwords remain in memory for the session and are never written to settings,
history, journals, or logs. Browsing uses a private temporary directory containing
the unlocked files, removed when the application exits normally.

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
Trash or removing retained backups can make a history entry unavailable. Archive
updates also participate in this history; archive undo/redo verifies whole-file
SHA-256 digests and retains both versions instead of using Trash.

**Secure delete…** in the file context menu's **More actions** performs a best-effort
overwrite of selected regular local files on Linux. It is also in the command palette
and can be assigned a shortcut; it has no default shortcut. Review checks the selection
without changing files and requires the acknowledgement checkbox before **Overwrite
and delete**; **Cancel** is the default. Folders, archive contents, symbolic links
(including links in the path), hard-linked files, and unsupported filesystems are rejected.
Commander rechecks the reviewed files, overwrites once with zeros, synchronizes writes,
reads back to verify the overwrite, then deletes them without Trash or Undo.
This does not guarantee unrecoverability on SSDs, flash storage, or filesystems with
snapshots or copy-on-write; backups and other copies are not erased.

Jobs shows secure-delete progress and verification, with pause and cancel controls.
Cancellation or failure cannot restore overwritten bytes: completed deletions remain
deleted, and remaining files may be partially overwritten. If an original filename
cannot be restored, the error reports the retained hidden path. Starting again requires
a new selection and review; secure-delete jobs do not offer Retry.

More → Recovery and saved operations opens durable operation records. A journal is synchronized
before publication, source removal, and other transfer mutations. If it cannot be
created, the operation fails before changing files. Interrupted jobs are surfaced on
startup; live jobs owned by another Commander instance are excluded. Recovery shows
completed destinations, uncertain steps, and retained originals. **Restore missing
originals** fills only absent paths and refuses changed backups. Existing destinations
are never overwritten by that recovery action. Archive operation details also offer
**Restore archive**, which restores the retained version only when the current
archive still matches that saved operation. It keeps the current version as another
recovery copy and adds an Undo entry. Retained originals and temporary outputs
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
Menu key, or `Shift+F10` opens the command palette (sidebar entries use Menu and
`Shift+F10` for their context menus); the palette supports type-to-filter,
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


## Power-user file tools

Open **More → File tools**, use the command palette, or assign shortcuts in Settings.

### Actionable search results

Search names, paths, or file contents with the existing search filters. Click to
select a result; use Ctrl-click or Shift-click to select several, including items
from different folders. Double-click or Enter opens the focused result. **Show in
folder** (Alt+Enter) closes Search and selects the result in its containing folder.

Right-click a result, press Shift+F10/Menu, or use **Actions** for file operations.
Right-clicking an unselected result selects it; clicking an already selected result
keeps the multi-selection. Menus show the item name or selection count, and capture
the paths and transfer destination when opened.

- Copy/cut files and copy their paths; paste into a selected folder.
- Copy or move the selection to the other pane using its existing job/conflict UI.
- Rename one item or open batch rename for multiple items.
- Move to Trash, or confirm permanent deletion of the captured selection.
- Open, choose an application, open a folder/archive in a new tab, or show it in the
  other pane. Available actions follow the same archive restrictions as file panels.

With the results focused, the configured keymap applies to clipboard actions,
selection, rename, cross-pane transfers, delete, undo, and redo. Ctrl+A selects all;
Enter and Alt+Enter retain the native open/reveal behavior. Menu arrows, Home, and
End move focus between actions. Keys typed in the query and filter fields stay in
those fields.

Changes made through Commander refresh the original search root and options while
Search is open. Unchanged selected paths stay selected when results reorder;
removed results disappear. Use the refresh button for changes made outside
Commander. Archive edits refresh after the replacement archive snapshot is ready.
A selected folder already covers its matching children for transfers, deletion,
and batch rename, so those children are not processed twice. File jobs and undo
reuse the normal operation engine; search actions leave unrelated pane selections
intact. Missing or inaccessible targets report errors through toasts.

### Batch rename preview

Select the items and choose **Batch rename**. The table shows each current name,
proposed name, and any conflicts. Rename is disabled for invalid names, duplicate
destinations, existing destination conflicts, and plans that change nothing.
Replacement supports case matching and regular expressions with `$1` / `${name}`
capture groups. Prefix/suffix, numbering templates (`{name}` and `{n}`), case
conversion, and preserving file extensions remain available. The engine checks the
plan again before writing and retains the existing undo/redo behavior.

### Duplicate finder

**Find duplicate files** scans the active folder recursively. Matching sizes are
compared using SHA-256 contents; names do not need to match. Hidden items are
optional. Symbolic links are not followed, and multiple hard links to the same
file are counted once. Results show groups, full paths, and redundant bytes.
Select a result and choose **Show folder** to review it using the
normal file operations. This tool never automatically deletes duplicates.

Scans run in the background and can be stopped. Unreadable or changed files are
skipped and counted; warning details appear in the summary tooltip. A scan is
limited to 500,000 regular files; choose a smaller root if that limit is reached.

### Visual file comparison

Select two files in one pane, or focus one file in each pane, then choose
**Compare files**. Editable path fields also let you choose another pair. Text
appears side by side with line numbers and highlighted additions/removals. Use
**Previous difference** and **Next difference** to move between changed groups.
Optional case and whitespace matching apply to text; binary files use hexadecimal
bytes with offsets. The current comparison view is read-only.

Each file is limited to 8 MiB and 100,000 text lines or hex rows. For very large
changed regions the comparison marks the differing middle as a group, keeping
matching prefixes and suffixes without an expensive full line alignment.

### Editing archives

Browse a ZIP, 7Z, TAR, TAR.GZ, or TGZ archive in either pane and use the normal
file commands. **F5 / Copy**, clipboard paste, and copy drops import files and
whole folder trees, including empty folders. Existing folders merge; name
conflicts use the normal Replace, Replace if Newer, Keep Both, and Skip dialog.
**F2 / Rename**, **F7 / New folder**, and **New file** operate within the current
archive folder. **Delete** asks to remove selected entries from the archive;
removing a directory includes its descendants. Copying entries out works as usual.

Each operation saves in the background and appears in Jobs with progress, pause,
and cancellation. There is no separate archive editor or Save step. The writer
builds a replacement beside the archive, checks that the original has not changed,
synchronizes the completed output, and publishes it by renaming. Cancellation or
a failure before publication keeps the original intact. A
`.commander-archive-backup-*` recovery file retains the original bytes beside the
archive; the activity log records its exact path. Keep it until you have checked
the updated archive, then remove it manually when no longer needed. Archive
updates are included in the normal Undo/Redo history, including after a restart.
Undo, redo, and recovery verify both the current archive and the retained copy by
SHA-256 before publishing a replacement. They refuse archives or backups whose
contents have changed. Each restore retains the version it replaces.

Open panels and tabs refresh to the new contents and retain existing folders where
possible. A removed folder falls back to the archive root. Updating an outer
archive invalidates nested archive views, which return to that outer archive.

Unchanged unencrypted ZIP entries retain their compressed data, permissions, and
archive comment. Encrypted ZIP entries are re-encrypted with AES-256 using the same
password; new files are encrypted too. Edited encrypted 7Z archives retain password
protection and hide file names. Undo, redo, and recovery restore the encrypted
archive bytes without storing the password. TAR headers and links are retained;
sparse or extended-metadata TARs are rejected instead of silently losing those
attributes. Archives with ambiguous duplicate entry paths and archives above
100,000 entries are not editable. Nested archives and files without write permission are shown as
read-only; copy a nested archive out to edit it. Imports accept regular files and
folders, not symbolic links or special files. Cut/move across archive boundaries,
batch rename, permissions changes, and automatic external-editor write-back are
not supported; use Copy and then Remove when moving entries.
