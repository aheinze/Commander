# ADR-0002: Use a direct virtualized model for details mode

- Status: Accepted
- Date: 2026-08-31
- Deciders: project maintainers

## Context

Details mode needs independently resizable name, size, modified, permissions, and
owner columns while keeping a 100k-entry listing virtualized. GTK's `ListView` is the
specified virtualization primitive, but it presents one composite row rather than
native sortable columns. `ColumnView` uses the same list-model and list-item-factory
machinery and provides the required details columns.

Wrapping the custom model in `SingleSelection` or `NoSelection` also made large model
changes more expensive. Selection itself belongs to M3 and must eventually be keyed by
stable file identity rather than a GTK row index.

## Decision

M2 details mode uses `gtk::ColumnView` backed by a custom GObject that implements both
`gio::ListModel` and a no-selection `gtk::SelectionModel`. The model owns one immutable
`Arc<Listing>` snapshot and materializes lightweight row objects only when GTK asks for
them. Lazy metadata updates compare the old and new snapshots and notify only changed
live rows.

The listing bridge publishes the first chunk immediately, waits for a presentation
acknowledgement, and coalesces queued snapshots before the next UI publication. This
prevents fast 100k-entry enumeration from starving GTK's first frame. M3 will extend or
replace the no-selection implementation with stable-identity selection without
changing the index/VFS boundary.

## Consequences

Details columns are native, accessible, resizable GTK columns and only viewport rows
own widgets. The app avoids per-entry GObjects and selection-adapter bookkeeping. The
custom model must obey `GListModel::items-changed` length invariants exactly, and M3
owns the additional work of implementing stable multi-selection. Compact and icon
views may share the same listing model through different GTK factories.

## Evidence

On the 100k-entry fixture in both panes, the M2 Wayland benchmark materialized 445 row
cells per pane during a steady scroll, kept RSS near 150 MiB, and measured row-bind
callbacks below 0.5 ms. The headless M1 gate remained at roughly 0.3 ms to first
snapshot and 63 ms for complete enumeration plus natural sorting.
