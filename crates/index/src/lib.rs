#![forbid(unsafe_code)]

//! Immutable directory snapshots, sorting, filtering, and watcher diffing.

mod error;
mod filter;
mod listing;
mod metadata;
mod natural;
mod watcher;

pub use error::IndexError;
pub use filter::{FilterResult, FilterStatus, FuzzyFilter, filter_listing};
pub use listing::{
    Listing, ListingEvent, ListingRequest, ListingTask, ListingTimings, RowIterator,
};
pub use metadata::{
    MetadataFailure, MetadataResult, hydrate_metadata, hydrate_metadata_range,
    hydrate_metadata_rows,
};
pub use watcher::{
    DirectoryWatcher, WatchApplyResult, WatchBatch, WatchChange, WatchChangeKind, apply_watch_batch,
};
