# ADR-0001: Preserve native filenames below the UI boundary

- Status: Accepted
- Date: 2026-08-31
- Deciders: project maintainers

## Context

The performance guidance asks directory entries to store a `Box<str>` filename, while
the required integration fixtures explicitly include invalid UTF-8 names. Unix paths
are byte sequences and converting such a name to UTF-8 is lossy. Reconstructing a path
from that display string could open, rename, or delete the wrong object.

## Decision

Headless crates store entry names as `Box<OsStr>` and paths as `std::path::PathBuf`
inside `VPath`. The listing still interns its parent path once, and a compile-time size
test keeps the lightweight `Entry` at or below the specified 64-byte budget. Normalized
UTF-8 keys used only for sorting and fuzzy matching are cached separately and never
used to address a filesystem object. The app will use `camino` only at UTF-8 UI and
configuration boundaries and will render invalid names with an explicit lossy-display
marker while retaining the native value for commands.

## Consequences

All native files remain addressable, including pathological Linux filenames. Each
entry's fat `OsStr` pointer has the same two-word representation as a boxed `str`, so
the hot structure remains compact. UI code must distinguish display labels from native
command paths, and non-Unix platforms may not expose stable file identities.

## Evidence

`dualpane-core` asserts the in-memory `Entry` size does not exceed 64 bytes. A native
platform test creates, enumerates, and compares a filename containing invalid UTF-8,
and M1 benchmarks measure the resulting 100k-entry listing representation rather than
assuming its cost.
