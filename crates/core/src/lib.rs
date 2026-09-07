#![forbid(unsafe_code)]

//! UI-independent domain primitives shared throughout Commander.

mod cancel;
mod entry;
mod path;
mod query;
mod selection;

pub use cancel::{CancelToken, Cancelled};
pub use entry::{
    Capabilities, CapabilitySupport, Entry, EntryKind, FileIdentity, Metadata, SizeHint, Timestamp,
};
pub use path::VPath;
pub use query::{Filter, SortDirection, SortKey, SortSpec};
pub use selection::{Selection, SelectionKey};
