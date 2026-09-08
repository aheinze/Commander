# Release checks

Run these checks against the exact working tree that will be packaged. A passing
build alone does not validate file operations or native interactions.

## Automated gates

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 -B -m unittest discover -s packaging -p 'test_*.py'
python3 scripts/test-native.py --backend wayland
cargo build --release --package dualpane-app --locked
```

The native runner requires Python 3, `dbus-run-session`, GNU `timeout`, and Mutter
with headless Wayland support. `--backend x11` instead uses Xvfb, `xvfb-run`, and
`xauth`. Build dependencies are the same GTK/libadwaita dependencies as the app.
An existing `PKG_CONFIG_PATH` is honored.

Every ignored app test is discovered automatically, except the FTP integration
tests run by the separate loopback fixture below. Each native test receives its own process,
display, private D-Bus session without host service activation, and temporary XDG
runtime. Configuration, cache, state, logs, and screenshots are isolated under a
new `target/native-tests/` directory, including a private copy of the test executable
so concurrent Cargo builds cannot replace it mid-run. Tests do not load the developer's Commander
session. Use `--filter search` to select names, or `--timeout 180` on a slow host.
An empty test selection fails. `results.json` records each exit status and duration;
CI uploads these artifacts even when a regression fails.

The native suite checks selection/reveal in all three views, Favorites labels,
activity controls, archive round trips, archive folder browsing (nested archives,
tabs, session restore, copy out, cancellation, and read-only inspector state),
secure-delete review and cancellation, captured selections, stale-file rejection,
and overwrite verification with disposable fixtures,
live filesystem updates, previews, remote
connection forms, saved remote editing, custom names and offline persistence, terminal tabs,
bundled icons, large search results, comparison
and sync review, settings Cancel/Apply, keyboard capture and conflict reassignment, shortcut persistence,
About diagnostics, tab folder menus (including inactive tabs, both panes, selection isolation,
and deferred archive/delete confirmations), formatted Markdown and heading links,
per-pane Git status (external branch changes, tab switches, non-repository folders,
and compact footers in light and dark appearances),
and both modern and GTK 4.14-compatible stylesheets. It runs
separately from `cargo test`, because GTK must initialize on one thread per process.

For the 100,000-entry latency gate, generate a new fixture once, then reuse it:

```console
cargo xtask gen-fixtures --root target/fixtures/flat-100k --files 100000 --files-per-directory 100000
cargo xtask check-m1-budgets --root target/fixtures/flat-100k/bucket-000000
```

## Release update checks

`cargo test --package dualpane-app updates::tests --locked` covers semantic version
ordering, stable-only selection, ETag reuse, rate-limit backoff, unsigned releases,
Ed25519 signature verification, architecture/glibc selection, corrupt downloads,
cancellation, and destination-file races. Packaging tests also reject mismatched
signing keys, incomplete inventories, and changed assets before signing.

Run `python3 scripts/test-native.py --backend wayland --filter gtk_updates` for the
About update controls, progress/error states, skipped versions, and cancellation
on dialog closure. These tests use fixtures and never contact GitHub or launch an
installer. Before publishing, configure the signing key described in the
[packaging guide](../packaging/README.md#update-signing-key), then check a signed
release from a previously installed build. Verify the saved package with
`sha256sum -c SHA256SUMS` and install it using the normal package manager or
AppImage workflow. Background checks and self-replacement are not yet implemented.

## Search guarantees

Search retains accessible matches when a child folder, file, or directory entry
cannot be read. An error toast reports incomplete coverage and the first error,
with Copy preserving the full diagnostic. The inline status retains the result
count and search progress. An inaccessible root or invalid query produces an
error toast instead of reporting an empty successful search.

Results are sorted and capped at 10,000 with an explicit limit toast. All returned
matches remain accessible through a virtualized list. Type, extension, size, and
date filters run before content reads. Content scans default to 8 MiB per file and
allow up to 64 MiB; larger files and binary files are not scanned. Symbolic-link
entries can be included, but linked directories are not traversed. Stop and dialog
closure invalidate late completions. Content reads check cancellation between
128 KiB chunks; a filesystem call already blocked in the OS must return first.

## Sidebar group regressions

Run `python3 scripts/test-native.py --backend wayland --filter gtk_sidebar` after
changes to sidebar groups. It covers saved collapsed state, keyboard ownership,
independent add controls, refreshed devices and favorites, and context menus.
Collapsed content must not take keyboard focus; Trash and section headers remain
reachable. The session round-trip and legacy-session tests cover state persistence
and expanded defaults for older sessions.

## Notification regressions

Run `python3 scripts/test-native.py --backend wayland --filter gtk_toasts` for the
shared notification gate. It checks replacement of inline pane errors, plain-text
rendering, redraw suppression, the four-toast queue, error priority, full diagnostic
copying, compact Unicode text, dialog overlays, debounced field validation, recovery
actions, and timeout dismissal. Light/dark and compact-dialog screenshots are saved
with the test artifacts. The current field tooltip must survive toast dismissal
and clear once the value is valid; closing a dialog must not emit late validation.

Exercise errors in search, comparison, previews, remote forms, settings updates,
custom tools, and recovery when changing those flows. Confirm feedback stays visible
above the active dialog and does not add inline error or information rows. Normal
counts/progress, static instructions, durable reports, and safety confirmations
must remain available. Keep the affected native workflow tests in the release gate.

## Reviewed folder synchronization

Run Compare directories from the command palette with the two pane folders open.
Choose the source direction, optionally compare file contents with SHA-256, and
review the complete action list before applying it. The list is virtualized rather
than truncated. Update preserves destination-only items; Mirror lists them as
Move to Trash. File/folder type conflicts and unsupported entries block the plan.

Metadata comparison uses file sizes and modification times. Content comparison
also detects different bytes with identical sizes and timestamps. Directories are
compared by their children; symbolic links by their targets, without traversal.
Unreadable entries fail the comparison rather than producing an incomplete plan.

Apply rechecks the reviewed snapshot before making changes and checks each target
again before its operation. Identical/nested roots and redirected parent symlinks
are rejected. Copies use staged writes, content verification, durable writes, and
retained replacement backups through the operation engine. Mirror removals run
after copies and recheck directory contents before moving them to Trash. Internal
recovery files are excluded; a folder containing one cannot be removed by Mirror.

A sync is not a transaction across the whole tree. On failure or cancellation,
already completed actions remain applied and the status reports how many finished.
Compare again before retrying. Snapshot checks reduce stale-plan risk but do not
lock the filesystem against other processes. Cancellation waits for an in-flight
filesystem call to return.

## Transfer failure and remote integration checks

Workspace tests inject disk-full errors, connection loss, flush/sync errors,
cancellation, and a process exit during copying. They cover replacement copies,
cross-device moves, readable nonseekable streams, readable recovery journals, and
retrying interrupted jobs. Assertions check source/original preservation and
temporary-file cleanup where the process is still alive.

The real FTP fixture additionally checks authenticated and anonymous mounts,
rejected credentials, opening a requested subfolder, a verified copy, and stopping
the server during a larger copy while preserving the original destination:

```console
python3 -m venv target/ftp-venv
target/ftp-venv/bin/pip install pyftpdlib==2.1.0
target/ftp-venv/bin/python -B scripts/test-ftp.py
```

Install the distro's GVfs FTP backend, GVfs FUSE bridge, FUSE 3, and D-Bus first
(Ubuntu: `gvfs gvfs-backends gvfs-fuse fuse3 dbus-daemon python3-venv`). The fixture
binds only to loopback, uses throwaway credentials and files, and starts a private
D-Bus session with service activation for GVfs. It unmounts only its own FUSE
mount. Logs and result JSON go to a new `target/ftp-tests/` directory. These two
tests are also enabled in CI; other advertised remote protocols still need their
own live integration coverage.

## Clean native package lifecycle

Build on the Ubuntu 24.04 baseline with `release.py`, then build a second revision
of the same binary to exercise the package manager's upgrade path:

```console
python3 release.py --formats deb,rpm --revision 1 --output target/packages-r1
python3 release.py --formats deb,rpm --revision 2 --output target/packages-r2
python3 -B scripts/test-packages.py --initial target/packages-r1 --upgrade target/packages-r2 --output target/package-checks
```

The runner uses rootless Podman by default; `--runtime docker` uses Docker. It
checks checksums, installs real dependencies in fresh Ubuntu 24.04 and Fedora 44
containers, launches under Xvfb, validates desktop/icon files, upgrades, launches
again, and removes the package while checking user-config retention. CSS parser
errors, invalid negative widget sizes, and critical runtime errors fail the launch check. All package installation
happens inside disposable containers. The output directory must be new; logs and
`results.json` record the distribution, image, duration, and exit code.

The release workflow runs this gate on both target architectures before publishing.
Containers exercise dependency resolution and package lifecycle; they do not
replace full desktop-session, graphics-driver, or older-version data-migration tests.

## Before a public release

Use [the packaging workflow](../packaging/README.md) to build and verify artifacts
for each supported architecture. Validate install, launch, upgrade, and uninstall
on clean machines for each claimed distribution; a developer-host build does not
cover dependency resolution on those systems. Check the packaged app's icons,
previews, keyboard navigation, and persistence in both supported desktop sessions.

Run the FTP/GVfs fixture and exercise the other advertised remote protocols against
disposable servers, including rejected credentials and a connection lost during
transfer. Keep remote integration and clean installation
results attached to the release being approved. These checks are not replaced by
the self-contained native suite.
