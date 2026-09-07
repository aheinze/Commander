# ADR-0003: Stable selection keys and pane-local filter workers

- Status: Accepted
- Date: 2026-08-31
- Deciders: project maintainers

## Context

M3 selection must survive sorting, filtering, and directory refreshes without binding
the domain model to GTK row numbers. Type-ahead must also respond below 30 ms for a
100k-entry listing. Rebuilding Nucleo's Unicode candidate store on every keystroke
costs more than the matching update itself and makes cancellation wasteful.

## Decision

Selection is a pane-local set of `SelectionKey` values. Local Unix entries use their
`(device, inode)` identity; entries without that identity fall back to their full
`VPath`. Filtered listings share the immutable source chunks and metadata with the
base listing and replace only the visible row-order array.

Each complete active-pane listing starts one cancellable worker that owns a reusable
`FuzzyFilter`. Query messages carry generations, are coalesced before publication,
and publish only immutable filtered snapshots. The inactive pane warms later or when
activated so its matcher cannot delay the active pane's first query. GTK receives no
per-entry filter objects.

## Consequences

Selection remains correct when row order changes and hidden selected entries reappear
after clearing a filter. Incremental matching reuses Nucleo's append optimization,
while navigation and refresh cancel the complete pane worker. The worker's candidate
store increases steady-state memory, but only for complete pane listings and without
duplicating entry or metadata storage.

The command-to-snapshot benchmark measures the active pane after matcher warm-up;
initial matcher construction overlaps directory presentation. Visible-row metadata
requests and divider updates are coalesced so they do not create a FIFO backlog ahead
of filter results.
