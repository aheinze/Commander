use std::cmp::Ordering;
use std::ffi::OsStr;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use dualpane_core::{CancelToken, Entry, Metadata, SortDirection, SortKey, SortSpec, VPath};
use dualpane_vfs::{Vfs, VfsError};

use crate::IndexError;
use crate::natural;

const LATER_CHUNK_ENTRIES: usize = 2_048;
const CHUNK_LATENCY: Duration = Duration::from_millis(16);

#[derive(Clone, Debug)]
pub(crate) struct IndexedEntry {
    entry: Entry,
    normalized_name: Box<str>,
}

impl IndexedEntry {
    fn new(entry: Entry) -> Self {
        let normalized_name = entry
            .name()
            .to_string_lossy()
            .to_lowercase()
            .into_boxed_str();
        Self {
            entry,
            normalized_name,
        }
    }
}

/// Immutable directory model published from a worker to consumers.
#[derive(Clone, Debug)]
pub struct Listing {
    parent: Arc<VPath>,
    chunks: Arc<[Arc<[IndexedEntry]>]>,
    chunk_offsets: Arc<[usize]>,
    row_order: Option<Arc<[u32]>>,
    metadata: Option<Arc<MetadataStore>>,
    sort: SortSpec,
    source_count: usize,
    complete: bool,
}

#[derive(Clone, Debug)]
struct MetadataStore {
    chunks: Arc<[Option<MetadataChunk>]>,
}

type MetadataChunk = Arc<[Option<Metadata>]>;

impl MetadataStore {
    fn get(&self, source_index: u32, chunk_offsets: &[usize]) -> Option<&Metadata> {
        let source_index = source_index as usize;
        let chunk_index = chunk_offsets
            .partition_point(|&offset| offset <= source_index)
            .checked_sub(1)?;
        let local_index = source_index.checked_sub(*chunk_offsets.get(chunk_index)?)?;
        self.chunks
            .get(chunk_index)?
            .as_ref()?
            .get(local_index)?
            .as_ref()
    }
}

impl Listing {
    pub(crate) fn from_entries_with_metadata(
        parent: Arc<VPath>,
        entries: Vec<Entry>,
        metadata: Vec<Option<Metadata>>,
        sort: SortSpec,
    ) -> Self {
        let chunk = entries
            .into_iter()
            .map(IndexedEntry::new)
            .collect::<Vec<_>>();
        Self::from_chunks(
            parent,
            vec![Arc::from(chunk)],
            Some(Arc::new(MetadataStore {
                chunks: Arc::from(vec![Some(Arc::from(metadata))]),
            })),
            sort,
            true,
        )
    }

    /// Reuses hydrated metadata from a filtered view of the same source snapshot.
    #[must_use]
    pub fn with_cached_metadata_from(&self, visible: &Self) -> Arc<Self> {
        let mut result = self.clone();
        if Arc::ptr_eq(&self.chunks, &visible.chunks) && visible.metadata.is_some() {
            result.metadata = visible.metadata.clone();
        }
        Arc::new(result)
    }

    /// Checks shared source storage and display order without walking directory rows.
    #[must_use]
    pub fn shares_rows_with(&self, previous: &Self) -> bool {
        Arc::ptr_eq(&self.chunks, &previous.chunks)
            && match (&self.row_order, &previous.row_order) {
                (Some(current), Some(old)) => Arc::ptr_eq(current, old),
                (None, None) => true,
                _ => false,
            }
    }

    /// Retains a filtered view's row order when the underlying entries are shared.
    #[must_use]
    pub fn with_view_order_from(&self, view: &Self) -> Option<Arc<Self>> {
        if !Arc::ptr_eq(&self.chunks, &view.chunks) || self.sort != view.sort {
            return None;
        }
        let mut result = self.clone();
        result.row_order = view.row_order.clone();
        Some(Arc::new(result))
    }

    #[cfg(test)]
    pub(crate) fn from_entries(parent: Arc<VPath>, entries: Vec<Entry>, sort: SortSpec) -> Self {
        let chunk: Arc<[IndexedEntry]> = entries
            .into_iter()
            .map(IndexedEntry::new)
            .collect::<Vec<_>>()
            .into_boxed_slice()
            .into();
        Self::from_chunks(parent, vec![chunk], None, sort, true)
    }

    fn from_chunks(
        parent: Arc<VPath>,
        chunks: Vec<Arc<[IndexedEntry]>>,
        metadata: Option<Arc<MetadataStore>>,
        sort: SortSpec,
        complete: bool,
    ) -> Self {
        let chunk_offsets: Vec<_> = chunks
            .iter()
            .scan(0_usize, |offset, chunk| {
                let current = *offset;
                *offset += chunk.len();
                Some(current)
            })
            .collect();
        let source_count = chunks.iter().map(|chunk| chunk.len()).sum();
        let row_order = complete.then(|| {
            sort_order(&chunks, &chunk_offsets, metadata.as_deref(), sort)
                .into_boxed_slice()
                .into()
        });

        Self {
            parent,
            chunks: chunks.into(),
            chunk_offsets: chunk_offsets.into(),
            row_order,
            metadata,
            sort,
            source_count,
            complete,
        }
    }

    pub(crate) fn with_metadata_updates(
        &self,
        updates: &[(u32, Metadata)],
        sort: SortSpec,
    ) -> Self {
        let mut metadata_chunks = self.metadata.as_ref().map_or_else(
            || vec![None; self.chunks.len()],
            |store| store.chunks.to_vec(),
        );
        let mut chunk_updates = vec![Vec::new(); self.chunks.len()];
        for (source_index, metadata) in updates {
            let source_index = *source_index as usize;
            if source_index >= self.source_count {
                continue;
            }
            let chunk_index = self
                .chunk_offsets
                .partition_point(|&offset| offset <= source_index)
                .saturating_sub(1);
            let local_index = source_index - self.chunk_offsets[chunk_index];
            chunk_updates[chunk_index].push((local_index, metadata));
        }
        for (chunk_index, updates) in chunk_updates.into_iter().enumerate() {
            if updates.is_empty() {
                continue;
            }
            let mut chunk = metadata_chunks[chunk_index].as_ref().map_or_else(
                || vec![None; self.chunks[chunk_index].len()],
                |values| values.to_vec(),
            );
            for (local_index, metadata) in updates {
                chunk[local_index] = Some(metadata.clone());
            }
            metadata_chunks[chunk_index] = Some(Arc::from(chunk.into_boxed_slice()));
        }
        let metadata = Arc::new(MetadataStore {
            chunks: Arc::from(metadata_chunks.into_boxed_slice()),
        });
        if sort == self.sort && matches!(sort.key, SortKey::Name | SortKey::Kind) {
            Self {
                parent: Arc::clone(&self.parent),
                chunks: Arc::clone(&self.chunks),
                chunk_offsets: Arc::clone(&self.chunk_offsets),
                row_order: self.row_order.clone(),
                metadata: Some(metadata),
                sort,
                source_count: self.source_count,
                complete: self.complete,
            }
        } else {
            let mut updated = Self::from_chunks(
                Arc::clone(&self.parent),
                self.chunks.iter().cloned().collect(),
                Some(metadata),
                sort,
                self.complete,
            );
            if let Some(visible_order) = &self.row_order {
                let mut visible = vec![false; self.source_count];
                for &source_index in visible_order.iter() {
                    visible[source_index as usize] = true;
                }
                let row_order = updated
                    .row_order
                    .as_deref()
                    .unwrap_or(visible_order)
                    .iter()
                    .copied()
                    .filter(|source_index| visible[*source_index as usize])
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
                    .into();
                updated.row_order = Some(row_order);
            }
            updated
        }
    }

    /// Returns the interned parent path shared by every entry.
    #[must_use]
    pub fn parent(&self) -> &VPath {
        &self.parent
    }

    /// Returns the number of enumerated entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.row_order
            .as_ref()
            .map_or(self.source_count, |order| order.len())
    }

    /// Returns the number of source entries retained by this snapshot.
    #[must_use]
    pub const fn source_len(&self) -> usize {
        self.source_count
    }

    /// Returns whether no entries have been enumerated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns whether enumeration and sorting are complete.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }

    /// Returns whether this snapshot only appends display rows to an earlier
    /// streamed snapshot.
    ///
    /// Incomplete listings are published from immutable source chunks.  This
    /// lets virtualized consumers retain their existing row objects and notify
    /// only the newly appended range instead of invalidating the whole model.
    #[must_use]
    pub fn is_append_only_successor_of(&self, previous: &Self) -> bool {
        if previous.complete
            || self.complete
            || self.parent != previous.parent
            || self.sort != previous.sort
            || self.source_count < previous.source_count
            || self.len() < previous.len()
            || previous.chunks.len() > self.chunks.len()
            || !previous
                .chunks
                .iter()
                .zip(self.chunks.iter())
                .all(|(old, new)| Arc::ptr_eq(old, new))
        {
            return false;
        }

        match (&previous.row_order, &self.row_order) {
            (None, None) => true,
            (Some(previous), Some(current)) => current.starts_with(previous),
            _ => false,
        }
    }

    /// Returns whether two completed snapshots represent the same directory
    /// rows and can safely share immutable source storage.
    #[must_use]
    pub fn has_same_rows_as(&self, other: &Self) -> bool {
        self.complete
            && other.complete
            && self.parent == other.parent
            && self.sort == other.sort
            && self.len() == other.len()
            && self
                .rows()
                .zip(other.rows())
                .all(|(left, right)| left == right)
    }

    /// Returns the sort used by this snapshot.
    #[must_use]
    pub const fn sort(&self) -> SortSpec {
        self.sort
    }

    /// Maps a visible row to the stable source index used by selection and metadata.
    #[must_use]
    pub fn source_index_at_row(&self, row: usize) -> Option<u32> {
        if row >= self.len() {
            return None;
        }
        self.row_order
            .as_ref()
            .map_or_else(|| u32::try_from(row).ok(), |order| order.get(row).copied())
    }

    /// Returns the entry displayed at a row.
    #[must_use]
    pub fn row(&self, row: usize) -> Option<&Entry> {
        let source_index = self.source_index_at_row(row)?;
        self.entry(source_index)
    }

    /// Returns an entry by stable source index.
    #[must_use]
    pub fn entry(&self, source_index: u32) -> Option<&Entry> {
        self.indexed_entry(source_index)
            .map(|indexed| &indexed.entry)
    }

    /// Returns metadata by stable source index when it has been fetched.
    #[must_use]
    pub fn metadata(&self, source_index: u32) -> Option<&Metadata> {
        self.metadata
            .as_ref()?
            .get(source_index, &self.chunk_offsets)
    }

    /// Iterates entries in display order without allocating.
    #[must_use]
    pub fn rows(&self) -> RowIterator<'_> {
        RowIterator {
            listing: self,
            next: 0,
        }
    }

    /// Publishes a new immutable row order while sharing entry and metadata storage.
    #[must_use]
    pub fn resort(&self, sort: SortSpec) -> Arc<Self> {
        Arc::new(Self::from_chunks(
            Arc::clone(&self.parent),
            self.chunks.iter().cloned().collect(),
            self.metadata.clone(),
            sort,
            self.complete,
        ))
    }

    /// Publishes a filtered or scored visible row order while sharing source storage.
    #[must_use]
    pub fn with_row_order(&self, row_order: Vec<u32>) -> Arc<Self> {
        assert!(
            row_order
                .iter()
                .all(|&source_index| (source_index as usize) < self.source_count),
            "row order contains an out-of-range source index"
        );
        Arc::new(Self {
            parent: Arc::clone(&self.parent),
            chunks: Arc::clone(&self.chunks),
            chunk_offsets: Arc::clone(&self.chunk_offsets),
            row_order: Some(row_order.into_boxed_slice().into()),
            metadata: self.metadata.clone(),
            sort: self.sort,
            source_count: self.source_count,
            complete: self.complete,
        })
    }

    /// Publishes a view that omits dot-prefixed entries while preserving the current order.
    #[must_use]
    pub fn without_hidden(&self) -> Arc<Self> {
        let row_order = (0..self.len())
            .filter_map(|row| self.source_index_at_row(row))
            .filter(|&source_index| {
                self.entry(source_index)
                    .is_some_and(|entry| !entry.name().as_encoded_bytes().starts_with(b"."))
            })
            .collect();
        self.with_row_order(row_order)
    }

    fn indexed_entry(&self, source_index: u32) -> Option<&IndexedEntry> {
        let source_index = source_index as usize;
        if source_index >= self.source_count {
            return None;
        }
        let chunk_index = self
            .chunk_offsets
            .partition_point(|&offset| offset <= source_index)
            .saturating_sub(1);
        let offset = self.chunk_offsets[chunk_index];
        self.chunks[chunk_index].get(source_index - offset)
    }
}

/// Borrowing iterator over visible listing rows.
pub struct RowIterator<'listing> {
    listing: &'listing Listing,
    next: usize,
}

impl<'listing> Iterator for RowIterator<'listing> {
    type Item = &'listing Entry;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.listing.row(self.next)?;
        self.next += 1;
        Some(entry)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.listing.len().saturating_sub(self.next);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for RowIterator<'_> {}

/// Tunables for one cancellable listing request.
#[derive(Clone, Debug)]
pub struct ListingRequest {
    pub path: VPath,
    pub sort: SortSpec,
}

impl ListingRequest {
    /// Creates a naturally sorted request for `path`.
    #[must_use]
    pub fn new(path: VPath) -> Self {
        Self {
            path,
            sort: SortSpec::default(),
        }
    }
}

/// Timings measured entirely inside the listing worker.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListingTimings {
    pub first_snapshot: Option<Duration>,
    pub enumeration: Duration,
    pub sorting: Duration,
    pub total: Duration,
}

/// Worker-to-consumer listing update.
#[derive(Debug)]
pub enum ListingEvent {
    Snapshot {
        listing: Arc<Listing>,
        elapsed: Duration,
    },
    Complete {
        listing: Arc<Listing>,
        timings: ListingTimings,
    },
    Cancelled,
    Failed(IndexError),
}

/// Owned listing worker. Dropping it cancels and joins the thread.
pub struct ListingTask {
    receiver: Receiver<ListingEvent>,
    cancel: CancelToken,
    worker: Option<JoinHandle<()>>,
}

impl ListingTask {
    /// Starts a listing on a named worker thread.
    ///
    /// # Errors
    ///
    /// Returns [`IndexError::Spawn`] if the worker thread cannot be created.
    pub fn spawn(vfs: Arc<dyn Vfs>, request: ListingRequest) -> Result<Self, IndexError> {
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        let (sender, receiver) = unbounded();
        let worker = thread::Builder::new()
            .name("dualpane-listing".to_owned())
            .spawn(move || run_listing(vfs.as_ref(), request, worker_cancel, &sender))
            .map_err(|source| IndexError::Spawn {
                worker: "listing",
                source,
            })?;

        Ok(Self {
            receiver,
            cancel,
            worker: Some(worker),
        })
    }

    /// Returns the event receiver for integration with an orchestration loop.
    #[must_use]
    pub const fn receiver(&self) -> &Receiver<ListingEvent> {
        &self.receiver
    }

    /// Requests cooperative cancellation.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Returns a cloneable cancellation handle for orchestration owners.
    #[must_use]
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Waits for a complete listing while still consuming intermediate snapshots.
    ///
    /// # Errors
    ///
    /// Returns the worker's error, cancellation, or a disconnected-channel failure.
    pub fn wait_complete(&self) -> Result<(Arc<Listing>, ListingTimings), IndexError> {
        loop {
            match self.receiver.recv() {
                Ok(ListingEvent::Complete { listing, timings }) => return Ok((listing, timings)),
                Ok(ListingEvent::Snapshot { .. }) => {}
                Ok(ListingEvent::Cancelled) => return Err(IndexError::Cancelled),
                Ok(ListingEvent::Failed(error)) => return Err(error),
                Err(_) => return Err(IndexError::WorkerPanicked),
            }
        }
    }

    /// Joins the worker and reports a panic.
    ///
    /// # Errors
    ///
    /// Returns [`IndexError::WorkerPanicked`] if the thread panicked.
    pub fn join(mut self) -> Result<(), IndexError> {
        self.join_inner()
    }

    fn join_inner(&mut self) -> Result<(), IndexError> {
        self.worker.take().map_or(Ok(()), |worker| {
            worker.join().map_err(|_| IndexError::WorkerPanicked)
        })
    }
}

impl Drop for ListingTask {
    fn drop(&mut self) {
        self.cancel.cancel();
        let _ = self.join_inner();
    }
}

fn run_listing(
    vfs: &dyn Vfs,
    request: ListingRequest,
    cancel: CancelToken,
    sender: &Sender<ListingEvent>,
) {
    let listing_span = tracing::info_span!("listing", path = %request.path);
    let _listing_guard = listing_span.enter();
    let started = Instant::now();
    let parent = Arc::new(request.path.clone());
    let mut chunks: Vec<Arc<[IndexedEntry]>> = Vec::new();
    let mut pending = Vec::with_capacity(LATER_CHUNK_ENTRIES);
    let mut last_publish = started;
    let mut first_snapshot = None;
    let iterator = match vfs.read_dir(&request.path, &cancel) {
        Ok(iterator) => iterator,
        Err(VfsError::Cancelled) => {
            let _ = sender.send(ListingEvent::Cancelled);
            return;
        }
        Err(error) => {
            let _ = sender.send(ListingEvent::Failed(error.into()));
            return;
        }
    };

    for result in iterator {
        if cancel.is_cancelled() {
            let _ = sender.send(ListingEvent::Cancelled);
            return;
        }
        let entry = match result {
            Ok(entry) => entry,
            Err(VfsError::Cancelled) => {
                let _ = sender.send(ListingEvent::Cancelled);
                return;
            }
            Err(error) => {
                let _ = sender.send(ListingEvent::Failed(error.into()));
                return;
            }
        };
        pending.push(IndexedEntry::new(entry));

        let latency_budget_expired = last_publish.elapsed() >= CHUNK_LATENCY;
        let later_chunk_full = first_snapshot.is_some() && pending.len() >= LATER_CHUNK_ENTRIES;
        if latency_budget_expired || later_chunk_full {
            flush_chunk(&mut chunks, &mut pending, LATER_CHUNK_ENTRIES);
            let elapsed = started.elapsed();
            first_snapshot.get_or_insert(elapsed);
            let listing = Arc::new(Listing::from_chunks(
                Arc::clone(&parent),
                chunks.clone(),
                None,
                request.sort,
                false,
            ));
            if sender
                .send(ListingEvent::Snapshot { listing, elapsed })
                .is_err()
            {
                return;
            }
            last_publish = Instant::now();
        }
    }

    let enumeration = started.elapsed();
    if !pending.is_empty() {
        let capacity = pending.len();
        flush_chunk(&mut chunks, &mut pending, capacity);
    }
    if cancel.is_cancelled() {
        let _ = sender.send(ListingEvent::Cancelled);
        return;
    }

    let sorting_started = Instant::now();
    let listing = {
        let sorting_span = tracing::info_span!("listing.sort", entries = chunks.len());
        let _sorting_guard = sorting_span.enter();
        Arc::new(Listing::from_chunks(
            parent,
            chunks,
            None,
            request.sort,
            true,
        ))
    };
    let sorting = sorting_started.elapsed();
    let timings = ListingTimings {
        first_snapshot,
        enumeration,
        sorting,
        total: started.elapsed(),
    };
    let _ = sender.send(ListingEvent::Complete { listing, timings });
}

fn flush_chunk(
    chunks: &mut Vec<Arc<[IndexedEntry]>>,
    pending: &mut Vec<IndexedEntry>,
    next_capacity: usize,
) {
    let replacement = Vec::with_capacity(next_capacity);
    let complete = std::mem::replace(pending, replacement);
    chunks.push(complete.into_boxed_slice().into());
}

fn sort_order(
    chunks: &[Arc<[IndexedEntry]>],
    chunk_offsets: &[usize],
    metadata: Option<&MetadataStore>,
    spec: SortSpec,
) -> Vec<u32> {
    let records: Vec<_> = chunks.iter().flat_map(|chunk| chunk.iter()).collect();
    let mut order: Vec<_> = (0..records.len())
        .map(|index| u32::try_from(index).expect("listing exceeds u32 index space"))
        .collect();
    order.sort_unstable_by(|&left, &right| {
        compare_records(
            records[left as usize],
            records[right as usize],
            metadata.and_then(|values| values.get(left, chunk_offsets)),
            metadata.and_then(|values| values.get(right, chunk_offsets)),
            spec,
        )
    });
    order
}

fn compare_records(
    left: &IndexedEntry,
    right: &IndexedEntry,
    left_metadata: Option<&Metadata>,
    right_metadata: Option<&Metadata>,
    spec: SortSpec,
) -> Ordering {
    if spec.directories_first {
        match right
            .entry
            .kind()
            .is_directory()
            .cmp(&left.entry.kind().is_directory())
        {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }

    let ordering = match spec.key {
        SortKey::Name => natural::compare(&left.normalized_name, &right.normalized_name),
        SortKey::Size => left_metadata
            .map_or(0, |metadata| metadata.size)
            .cmp(&right_metadata.map_or(0, |metadata| metadata.size)),
        SortKey::Modified => left_metadata
            .and_then(|metadata| metadata.modified)
            .cmp(&right_metadata.and_then(|metadata| metadata.modified)),
        SortKey::Kind => left.entry.kind().cmp(&right.entry.kind()),
    };
    let ordering = match spec.direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    };
    if ordering == Ordering::Equal {
        natural::compare(&left.normalized_name, &right.normalized_name)
            .then_with(|| raw_name(left.entry.name()).cmp(raw_name(right.entry.name())))
    } else {
        ordering
    }
}

fn raw_name(name: &OsStr) -> &[u8] {
    name.as_encoded_bytes()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use dualpane_core::{CancelToken, Capabilities, Entry, EntryKind, Metadata, SizeHint, VPath};
    use dualpane_vfs::{LocalFs, ReadSeek, Result as VfsResult, Vfs, VfsError, WriteSeek};
    use tempfile::tempdir;

    use super::{ListingEvent, ListingRequest, ListingTask};
    use crate::IndexError;

    #[test]
    fn fast_small_listing_publishes_once_after_natural_sort() {
        let fixture = tempdir().expect("fixture");
        for index in 0..250 {
            fs::write(fixture.path().join(format!("file{index}.txt")), []).expect("fixture file");
        }
        let request = ListingRequest::new(VPath::from(fixture.path()));
        let task = ListingTask::spawn(Arc::new(LocalFs), request).expect("spawn listing");

        let first = task
            .receiver()
            .recv_timeout(Duration::from_secs(2))
            .expect("first listing event");
        let ListingEvent::Complete { listing, timings } = first else {
            panic!("expected the completed listing without a redundant snapshot");
        };
        assert_eq!(listing.len(), 250);
        assert!(listing.is_complete());
        assert!(timings.first_snapshot.is_none());
        let names: Vec<_> = listing
            .rows()
            .take(12)
            .map(|entry| entry.name().to_string_lossy().into_owned())
            .collect();
        insta::assert_debug_snapshot!(names, @r###"
        [
            "file0.txt",
            "file1.txt",
            "file2.txt",
            "file3.txt",
            "file4.txt",
            "file5.txt",
            "file6.txt",
            "file7.txt",
            "file8.txt",
            "file9.txt",
            "file10.txt",
            "file11.txt",
        ]
        "###);
        task.join().expect("join listing");
    }

    #[test]
    fn slow_listing_still_streams_within_the_latency_budget() {
        let task = ListingTask::spawn(
            Arc::new(SlowVfs),
            ListingRequest::new(VPath::from("virtual")),
        )
        .expect("spawn listing");

        let first = task
            .receiver()
            .recv_timeout(Duration::from_secs(2))
            .expect("first listing event");
        let ListingEvent::Snapshot { listing, elapsed } = first else {
            panic!("expected an intermediate snapshot");
        };
        assert!(!listing.is_empty());
        assert!(!listing.is_complete());
        assert!(elapsed < Duration::from_millis(100));

        task.cancel();
        assert!(matches!(task.wait_complete(), Err(IndexError::Cancelled)));
        task.join().expect("join cancelled listing");
    }

    #[test]
    fn cancelling_a_listing_joins_its_worker() {
        let task = ListingTask::spawn(
            Arc::new(SlowVfs),
            ListingRequest::new(VPath::from("virtual")),
        )
        .expect("spawn listing");
        task.cancel();

        assert!(matches!(task.wait_complete(), Err(IndexError::Cancelled)));
        task.join().expect("join cancelled worker");
    }

    #[test]
    fn filtered_row_order_keeps_complete_source_storage() {
        let parent = Arc::new(VPath::from("virtual"));
        let entries = ["one", "two", "three"]
            .into_iter()
            .map(|name| Entry::new(name.into(), EntryKind::File, None))
            .collect();
        let listing = super::Listing::from_entries(parent, entries, Default::default());

        let filtered = listing.with_row_order(vec![2, 0]);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered.source_len(), 3);
        assert_eq!(filtered.row(0).unwrap().name(), "three");
        assert_eq!(filtered.row(1).unwrap().name(), "one");
        assert_eq!(filtered.entry(1).unwrap().name(), "two");
    }

    #[test]
    fn hidden_view_preserves_visible_sort_order_and_source_storage() {
        let parent = Arc::new(VPath::from("virtual"));
        let entries = ["two", ".hidden", "one"]
            .into_iter()
            .map(|name| Entry::new(name.into(), EntryKind::File, None))
            .collect();
        let listing = super::Listing::from_entries(parent, entries, Default::default());

        let visible = listing.without_hidden();
        let names: Vec<_> = visible
            .rows()
            .map(|entry| entry.name().to_string_lossy().into_owned())
            .collect();

        assert_eq!(names, ["one", "two"]);
        assert_eq!(visible.source_len(), 3);
        assert_eq!(visible.entry(1).unwrap().name(), ".hidden");
    }

    #[test]
    fn streamed_snapshots_report_append_only_successors() {
        let parent = Arc::new(VPath::from("virtual"));
        let first: Arc<[super::IndexedEntry]> = ["one", ".hidden"]
            .into_iter()
            .map(|name| super::IndexedEntry::new(Entry::new(name.into(), EntryKind::File, None)))
            .collect::<Vec<_>>()
            .into();
        let second: Arc<[super::IndexedEntry]> = ["two"]
            .into_iter()
            .map(|name| super::IndexedEntry::new(Entry::new(name.into(), EntryKind::File, None)))
            .collect::<Vec<_>>()
            .into();
        let earlier = super::Listing::from_chunks(
            Arc::clone(&parent),
            vec![Arc::clone(&first)],
            None,
            Default::default(),
            false,
        );
        let later = super::Listing::from_chunks(
            parent,
            vec![first, second],
            None,
            Default::default(),
            false,
        );

        assert!(later.is_append_only_successor_of(&earlier));
        assert!(
            later
                .without_hidden()
                .is_append_only_successor_of(&earlier.without_hidden())
        );

        let completed = super::Listing {
            complete: true,
            ..later
        };
        assert!(!completed.is_append_only_successor_of(&earlier));
    }

    #[test]
    fn equivalent_completed_listings_can_share_source_storage() {
        let entries = || {
            ["one", "two"]
                .into_iter()
                .map(|name| Entry::new(name.into(), EntryKind::File, None))
                .collect()
        };
        let first = super::Listing::from_entries(
            Arc::new(VPath::from("virtual")),
            entries(),
            Default::default(),
        );
        let second = super::Listing::from_entries(
            Arc::new(VPath::from("virtual")),
            entries(),
            Default::default(),
        );

        assert!(first.has_same_rows_as(&second));
    }

    struct SlowVfs;

    impl Vfs for SlowVfs {
        fn read_dir(
            &self,
            _path: &VPath,
            _cancel: &CancelToken,
        ) -> VfsResult<Box<dyn Iterator<Item = VfsResult<Entry>> + Send>> {
            Ok(Box::new((0..10_000).map(|index| {
                thread::sleep(Duration::from_micros(100));
                Ok(Entry::new(
                    format!("entry-{index}").into(),
                    EntryKind::File,
                    None,
                ))
            })))
        }

        fn stat(&self, path: &VPath, _follow: bool) -> VfsResult<Metadata> {
            Err(test_error("stat", path))
        }

        fn open_read(&self, path: &VPath) -> VfsResult<Box<dyn ReadSeek>> {
            Err(test_error("open", path))
        }

        fn create_write(&self, path: &VPath, _hint: SizeHint) -> VfsResult<Box<dyn WriteSeek>> {
            Err(test_error("create", path))
        }

        fn rename(&self, from: &VPath, _to: &VPath) -> VfsResult<()> {
            Err(test_error("rename", from))
        }

        fn remove(&self, path: &VPath, _kind: EntryKind) -> VfsResult<()> {
            Err(test_error("remove", path))
        }

        fn capabilities(&self) -> Capabilities {
            LocalFs.capabilities()
        }
    }

    fn test_error(operation: &'static str, path: &VPath) -> VfsError {
        VfsError::Io {
            operation,
            path: path.clone(),
            source: io::Error::other("not implemented by test VFS"),
        }
    }
}
