use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use dualpane_core::{CancelToken, Entry, VPath};
use dualpane_vfs::{Vfs, VfsError};
use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{IndexError, Listing};

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(50);

/// Coalesced change category for one direct child of a watched directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchChangeKind {
    Created,
    Changed,
    Removed,
}

/// One coalesced path change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchChange {
    pub path: VPath,
    pub kind: WatchChangeKind,
}

/// A 50ms watch-event batch. Queue overflow is represented explicitly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchBatch {
    pub changes: Vec<WatchChange>,
    pub rescan_required: bool,
    pub errors: Vec<String>,
}

/// Result of applying a watcher batch without relisting the directory.
#[derive(Clone, Debug)]
pub enum WatchApplyResult {
    Updated(Arc<Listing>),
    RescanRequired,
}

/// One non-recursive native watcher with an owned debounce worker.
pub struct DirectoryWatcher {
    watcher: RecommendedWatcher,
    watched_path: PathBuf,
    receiver: Receiver<WatchBatch>,
    cancel: CancelToken,
    worker: Option<JoinHandle<()>>,
}

impl DirectoryWatcher {
    /// Starts one non-recursive watch and a 50ms coalescing worker.
    ///
    /// # Errors
    ///
    /// Returns watcher setup or worker-spawn failures.
    pub fn new(path: &VPath) -> Result<Self, IndexError> {
        let (raw_sender, raw_receiver) = unbounded();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = raw_sender.send(event);
        })
        .map_err(|error| watch_error("create watcher", path, error))?;
        watcher
            .watch(path.as_path(), RecursiveMode::NonRecursive)
            .map_err(|error| watch_error("watch directory", path, error))?;

        let (sender, receiver) = unbounded();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        let watched_path = path.as_path().to_path_buf();
        let worker_path = watched_path.clone();
        let worker = thread::Builder::new()
            .name("dualpane-watch-debounce".to_owned())
            .spawn(move || debounce_loop(&worker_path, raw_receiver, sender, worker_cancel))
            .map_err(|source| IndexError::Spawn {
                worker: "watch debounce",
                source,
            })?;

        Ok(Self {
            watcher,
            watched_path,
            receiver,
            cancel,
            worker: Some(worker),
        })
    }

    /// Returns coalesced batches for the orchestration layer.
    #[must_use]
    pub const fn receiver(&self) -> &Receiver<WatchBatch> {
        &self.receiver
    }

    /// Requests bounded cooperative shutdown.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    fn join_inner(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for DirectoryWatcher {
    fn drop(&mut self) {
        self.cancel.cancel();
        let _ = self.watcher.unwatch(&self.watched_path);
        self.join_inner();
    }
}

/// Applies direct-child changes to an immutable snapshot. It never enumerates the directory.
///
/// # Errors
///
/// Returns cancellation or an unexpected metadata error. A raced deletion is treated as a
/// remove, while queue overflow asks the caller to perform one fresh listing.
pub fn apply_watch_batch(
    vfs: &dyn Vfs,
    listing: &Arc<Listing>,
    batch: &WatchBatch,
    cancel: &CancelToken,
) -> Result<WatchApplyResult, IndexError> {
    cancel.check().map_err(|_| IndexError::Cancelled)?;
    if batch.rescan_required {
        return Ok(WatchApplyResult::RescanRequired);
    }

    if batch.changes.is_empty() {
        return Ok(WatchApplyResult::Updated(Arc::clone(listing)));
    }
    // Index once per batch instead of scanning all rows for each changed name.
    let positions = (0..listing.source_len() as u32)
        .filter_map(|index| listing.entry(index).map(|entry| (entry.name(), index)))
        .collect::<HashMap<_, _>>();
    let mut changed = BTreeMap::new();
    let mut metadata_updates = Vec::new();
    let mut structural = false;
    for change in &batch.changes {
        cancel.check().map_err(|_| IndexError::Cancelled)?;
        if change.path.parent().as_ref() != Some(listing.parent()) {
            continue;
        }
        let Some(name) = change.path.file_name() else {
            continue;
        };
        let existing = positions.get(name).copied();
        // Observe the final state, including a delete followed by a rapid recreation.
        match vfs.stat(&change.path, false) {
            Ok(metadata) => {
                let updated = Entry::new(name.to_owned(), metadata.kind, metadata.identity);
                structural |= existing.and_then(|index| listing.entry(index)) != Some(&updated);
                if let Some(index) = existing {
                    metadata_updates.push((index, metadata.clone()));
                }
                changed.insert(name.to_owned(), Some((updated, metadata)));
            }
            Err(error) if error.io_kind() == Some(io::ErrorKind::NotFound) => {
                structural |= existing.is_some();
                changed.insert(name.to_owned(), None);
            }
            Err(VfsError::Cancelled) => return Err(IndexError::Cancelled),
            Err(error) => return Err(error.into()),
        }
    }
    cancel.check().map_err(|_| IndexError::Cancelled)?;
    if !structural {
        return Ok(WatchApplyResult::Updated(Arc::new(
            listing.with_metadata_updates(&metadata_updates, listing.sort()),
        )));
    }
    let mut entries = Vec::with_capacity(listing.source_len() + changed.len());
    let mut metadata = Vec::with_capacity(entries.capacity());
    for index in 0..listing.source_len() as u32 {
        cancel.check().map_err(|_| IndexError::Cancelled)?;
        let entry = listing.entry(index).expect("source entry exists");
        match changed.remove(entry.name()) {
            Some(Some((entry, value))) => {
                entries.push(entry);
                metadata.push(Some(value));
            }
            Some(None) => {}
            None => {
                entries.push(entry.clone());
                metadata.push(listing.metadata(index).cloned());
            }
        }
    }
    for (entry, value) in changed.into_values().flatten() {
        entries.push(entry);
        metadata.push(Some(value));
    }
    let updated = Listing::from_entries_with_metadata(
        Arc::new(listing.parent().clone()),
        entries,
        metadata,
        listing.sort(),
    );
    Ok(WatchApplyResult::Updated(Arc::new(updated)))
}

fn debounce_loop(
    watched_path: &Path,
    receiver: Receiver<notify::Result<Event>>,
    sender: Sender<WatchBatch>,
    cancel: CancelToken,
) {
    while !cancel.is_cancelled() {
        let first = match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let deadline = Instant::now() + DEBOUNCE_WINDOW;
        let mut events = vec![first];

        while !cancel.is_cancelled() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match receiver.recv_timeout(remaining) {
                Ok(event) => events.push(event),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }

        if cancel.is_cancelled() {
            return;
        }
        let batch = coalesce(watched_path, events);
        if sender.send(batch).is_err() {
            return;
        }
    }
}

fn coalesce(watched_path: &Path, events: Vec<notify::Result<Event>>) -> WatchBatch {
    let mut changes = BTreeMap::new();
    let mut rescan_required = false;
    let mut errors = Vec::new();

    for event in events {
        match event {
            Ok(event) => {
                rescan_required |= event.need_rescan();
                let Some(kind) = change_kind(event.kind) else {
                    continue;
                };
                for path in event.paths {
                    if path.parent() != Some(watched_path) {
                        continue;
                    }
                    changes
                        .entry(path)
                        .and_modify(|current| *current = merge_change(*current, kind))
                        .or_insert(kind);
                }
            }
            Err(error) => {
                rescan_required = true;
                errors.push(error.to_string());
            }
        }
    }

    WatchBatch {
        changes: changes
            .into_iter()
            .map(|(path, kind)| WatchChange {
                path: VPath::from(path),
                kind,
            })
            .collect(),
        rescan_required,
        errors,
    }
}

fn change_kind(kind: EventKind) -> Option<WatchChangeKind> {
    match kind {
        EventKind::Create(_) => Some(WatchChangeKind::Created),
        EventKind::Modify(_) => Some(WatchChangeKind::Changed),
        EventKind::Remove(_) => Some(WatchChangeKind::Removed),
        EventKind::Any | EventKind::Other => Some(WatchChangeKind::Changed),
        EventKind::Access(_) => None,
    }
}

fn merge_change(current: WatchChangeKind, next: WatchChangeKind) -> WatchChangeKind {
    match (current, next) {
        (WatchChangeKind::Created, WatchChangeKind::Changed)
        | (WatchChangeKind::Changed, WatchChangeKind::Created) => WatchChangeKind::Created,
        (WatchChangeKind::Removed, WatchChangeKind::Created) => WatchChangeKind::Changed,
        (_, WatchChangeKind::Removed) => WatchChangeKind::Removed,
        (_, kind) => kind,
    }
}

fn watch_error(operation: &'static str, path: &VPath, error: notify::Error) -> IndexError {
    IndexError::Watch {
        operation,
        path: path.clone(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    use dualpane_core::{CancelToken, Entry, EntryKind, SortSpec, VPath};
    use dualpane_vfs::LocalFs;
    use tempfile::tempdir;

    use crate::Listing;

    use super::{
        DirectoryWatcher, WatchApplyResult, WatchBatch, WatchChange, WatchChangeKind,
        apply_watch_batch,
    };

    struct StatsOnly(std::sync::atomic::AtomicUsize);

    impl dualpane_vfs::Vfs for StatsOnly {
        fn stat(
            &self,
            path: &VPath,
            follow: bool,
        ) -> dualpane_vfs::Result<dualpane_core::Metadata> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            dualpane_vfs::Vfs::stat(&LocalFs, path, follow)
        }
        fn read_dir(
            &self,
            _: &VPath,
            _: &CancelToken,
        ) -> dualpane_vfs::Result<Box<dyn Iterator<Item = dualpane_vfs::Result<Entry>> + Send>>
        {
            panic!("incremental updates must not enumerate directories")
        }
        fn open_read(&self, _: &VPath) -> dualpane_vfs::Result<Box<dyn dualpane_vfs::ReadSeek>> {
            panic!("unexpected read")
        }
        fn create_write(
            &self,
            _: &VPath,
            _: dualpane_core::SizeHint,
        ) -> dualpane_vfs::Result<Box<dyn dualpane_vfs::WriteSeek>> {
            panic!("unexpected write")
        }
        fn rename(&self, _: &VPath, _: &VPath) -> dualpane_vfs::Result<()> {
            panic!("unexpected mutation")
        }
        fn remove(&self, _: &VPath, _: EntryKind) -> dualpane_vfs::Result<()> {
            panic!("unexpected mutation")
        }
        fn capabilities(&self) -> dualpane_core::Capabilities {
            dualpane_vfs::Vfs::capabilities(&LocalFs)
        }
    }

    fn batch(parent: &VPath, names: &[&str]) -> WatchBatch {
        WatchBatch {
            changes: names
                .iter()
                .map(|name| WatchChange {
                    path: parent.join_name(name.as_ref()),
                    kind: WatchChangeKind::Changed,
                })
                .collect(),
            rescan_required: false,
            errors: Vec::new(),
        }
    }

    #[test]
    fn one_edit_in_100k_rows_stats_only_one_file_and_reuses_rows() {
        use dualpane_vfs::Vfs;
        let fixture = tempdir().unwrap();
        let parent = VPath::from(fixture.path());
        let path = parent.join_name("target".as_ref());
        fs::write(path.as_path(), b"before").unwrap();
        let metadata = LocalFs.stat(&path, false).unwrap();
        let mut entries = (0..100_000)
            .map(|index| Entry::new(format!("file-{index:06}").into(), EntryKind::File, None))
            .collect::<Vec<_>>();
        entries.push(Entry::new(
            "target".into(),
            EntryKind::File,
            metadata.identity,
        ));
        let listing = Arc::new(Listing::from_entries(
            Arc::new(parent.clone()),
            entries,
            SortSpec::default(),
        ));
        fs::write(path.as_path(), b"after, with a different length").unwrap();
        let vfs = StatsOnly(Default::default());
        let started = std::time::Instant::now();
        let WatchApplyResult::Updated(updated) = apply_watch_batch(
            &vfs,
            &listing,
            &batch(&parent, &["target"]),
            &CancelToken::new(),
        )
        .unwrap() else {
            panic!("unexpected rescan")
        };
        eprintln!(
            "100k-row edit: {:.2} ms, one stat, no enumeration",
            started.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(vfs.0.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(updated.shares_rows_with(&listing));
        assert_eq!(updated.metadata(100_000).unwrap().size, 30);
    }

    #[test]
    fn rename_and_size_changes_preserve_other_metadata_and_sort_order() {
        use dualpane_core::SortKey;
        let fixture = tempdir().unwrap();
        for (name, length) in [("a", 2), ("b", 8), ("c", 4)] {
            fs::write(fixture.path().join(name), vec![b'x'; length]).unwrap();
        }
        let parent = VPath::from(fixture.path());
        let sort = SortSpec {
            key: SortKey::Size,
            ..SortSpec::default()
        };
        let listing = crate::ListingTask::spawn(
            Arc::new(LocalFs),
            crate::ListingRequest {
                path: parent.clone(),
                sort,
            },
        )
        .unwrap()
        .wait_complete()
        .unwrap()
        .0;
        let listing = crate::hydrate_metadata(&LocalFs, &listing, &CancelToken::new(), sort)
            .unwrap()
            .listing;
        let b = (0..listing.source_len() as u32)
            .find(|&i| listing.entry(i).unwrap().name() == "b")
            .unwrap();
        let cached = listing.metadata(b).unwrap().clone();
        fs::write(fixture.path().join("a"), [b'x'; 20]).unwrap();
        fs::rename(fixture.path().join("c"), fixture.path().join("d")).unwrap();
        let WatchApplyResult::Updated(updated) = apply_watch_batch(
            &StatsOnly(Default::default()),
            &listing,
            &batch(&parent, &["a", "c", "d"]),
            &CancelToken::new(),
        )
        .unwrap() else {
            panic!("unexpected rescan")
        };
        assert_eq!(
            updated
                .rows()
                .map(|entry| entry.name().to_str().unwrap())
                .collect::<Vec<_>>(),
            ["d", "b", "a"]
        );
        assert_eq!(
            updated.metadata(updated.source_index_at_row(1).unwrap()),
            Some(&cached)
        );
        assert_eq!(
            updated
                .metadata(updated.source_index_at_row(2).unwrap())
                .unwrap()
                .size,
            20
        );
    }

    #[test]
    fn late_delete_events_observe_a_recreated_file() {
        let fixture = tempdir().unwrap();
        let parent = VPath::from(fixture.path());
        let listing = Arc::new(Listing::from_entries(
            Arc::new(parent.clone()),
            Vec::new(),
            SortSpec::default(),
        ));
        fs::write(fixture.path().join("recreated"), b"new").unwrap();
        let mut change = batch(&parent, &["recreated"]);
        change.changes[0].kind = WatchChangeKind::Removed;
        let WatchApplyResult::Updated(updated) =
            apply_watch_batch(&LocalFs, &listing, &change, &CancelToken::new()).unwrap()
        else {
            panic!("unexpected rescan")
        };
        assert_eq!(updated.len(), 1);
        assert_eq!(updated.row(0).unwrap().name(), "recreated");
    }

    #[test]
    fn synthetic_diff_updates_only_named_children() {
        let fixture = tempdir().expect("fixture");
        fs::write(fixture.path().join("kept"), b"updated").expect("kept");
        fs::write(fixture.path().join("added"), b"new").expect("added");
        let parent = VPath::from(fixture.path());
        let listing = Arc::new(Listing::from_entries(
            Arc::new(parent.clone()),
            vec![
                Entry::new("kept".into(), EntryKind::File, None),
                Entry::new("removed".into(), EntryKind::File, None),
            ],
            SortSpec::default(),
        ));
        let batch = WatchBatch {
            changes: vec![
                WatchChange {
                    path: parent.join_name("removed".as_ref()),
                    kind: WatchChangeKind::Removed,
                },
                WatchChange {
                    path: parent.join_name("added".as_ref()),
                    kind: WatchChangeKind::Created,
                },
            ],
            rescan_required: false,
            errors: Vec::new(),
        };

        let result =
            apply_watch_batch(&LocalFs, &listing, &batch, &CancelToken::new()).expect("apply diff");
        let WatchApplyResult::Updated(updated) = result else {
            panic!("unexpected rescan");
        };
        let names: Vec<_> = updated
            .rows()
            .map(|entry| entry.name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["added", "kept"]);
    }

    #[test]
    fn native_watcher_coalesces_changes() {
        let fixture = tempdir().expect("fixture");
        let path = VPath::from(fixture.path());
        let watcher = DirectoryWatcher::new(&path).expect("watcher");
        let created = fixture.path().join("created.txt");

        fs::write(&created, b"first").expect("create file");
        fs::write(&created, b"second").expect("modify file");

        let batch = watcher
            .receiver()
            .recv_timeout(Duration::from_secs(3))
            .expect("watch batch");
        assert!(!batch.rescan_required, "watch errors: {:?}", batch.errors);
        assert!(
            batch
                .changes
                .iter()
                .any(|change| change.path.as_path() == created)
        );
    }
}
