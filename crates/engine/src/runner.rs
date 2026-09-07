use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use dualpane_core::VPath;
use dualpane_vfs::{LocalFs, Vfs};

use crate::{
    Conflict, ConflictDecision, ConflictId, ConflictResponse, JobControl, JobEvent, JobId, JobKind,
    JobState, JobSummary, ProgressEmitter, ScanOptions, TransferOptions, TransferOutcome,
    copy_plan, delete_permanently, move_sources, scan_sources, trash_sources,
};

/// Headless concurrent operation service. Every spawned job owns one worker thread.
#[derive(Clone)]
pub struct OperationEngine {
    vfs: Arc<dyn Vfs>,
    journal_directory: Option<std::path::PathBuf>,
}

impl OperationEngine {
    #[must_use]
    pub fn new(vfs: Arc<dyn Vfs>) -> Self {
        Self {
            vfs,
            journal_directory: None,
        }
    }

    /// Enable durable recovery records and retained overwrite backups for every job.
    #[must_use]
    pub fn with_journal_directory(mut self, directory: std::path::PathBuf) -> Self {
        self.journal_directory = Some(directory);
        self
    }

    #[must_use]
    pub fn spawn_copy(
        &self,
        sources: Vec<VPath>,
        destination: VPath,
        scan_options: ScanOptions,
        transfer_options: TransferOptions,
    ) -> JobHandle {
        let vfs = Arc::clone(&self.vfs);
        spawn(
            JobKind::Copy,
            self.journal_directory.clone(),
            sources.clone(),
            Some(destination.clone()),
            move |id, control, events, decisions| {
                send_state(&events, id, JobState::Scanning);
                let plan = scan_sources(&*vfs, &sources, scan_options, &control, |_| {})?;
                send_state(&events, id, JobState::Running);
                let mut emitter = ProgressEmitter::new(
                    id,
                    events.clone(),
                    (plan.total_bytes, plan.entries.len() as u64),
                );
                let outcome = copy_plan(
                    &*vfs,
                    &plan,
                    &destination,
                    transfer_options,
                    &control,
                    |progress| {
                        emitter.advanced(
                            progress.bytes,
                            progress.items_finished,
                            Some(progress.path),
                        );
                    },
                    |state| send_state(&events, id, state),
                    |conflict| await_conflict(id, conflict, &control, &events, &decisions),
                )?;
                emitter.flush();
                Ok(outcome)
            },
        )
    }

    #[must_use]
    pub fn spawn_move(
        &self,
        sources: Vec<VPath>,
        destination: VPath,
        scan_options: ScanOptions,
        transfer_options: TransferOptions,
    ) -> JobHandle {
        let vfs = Arc::clone(&self.vfs);
        spawn(
            JobKind::Move,
            self.journal_directory.clone(),
            sources.clone(),
            Some(destination.clone()),
            move |id, control, events, decisions| {
                send_state(&events, id, JobState::Scanning);
                let plan = scan_sources(&*vfs, &sources, scan_options, &control, |_| {})?;
                send_state(&events, id, JobState::Running);
                let mut emitter = ProgressEmitter::new(
                    id,
                    events.clone(),
                    (plan.total_bytes, plan.entries.len() as u64),
                );
                let outcome = move_sources(
                    &*vfs,
                    &sources,
                    &destination,
                    scan_options,
                    transfer_options,
                    &control,
                    |progress| {
                        emitter.advanced(
                            progress.bytes,
                            progress.items_finished,
                            Some(progress.path),
                        );
                    },
                    |state| send_state(&events, id, state),
                    |conflict| await_conflict(id, conflict, &control, &events, &decisions),
                )?;
                emitter.flush();
                Ok(outcome)
            },
        )
    }

    #[must_use]
    pub fn spawn_trash(&self, sources: Vec<VPath>) -> JobHandle {
        let vfs = Arc::clone(&self.vfs);
        spawn(
            JobKind::Trash,
            self.journal_directory.clone(),
            sources.clone(),
            None,
            move |id, control, events, _| {
                send_state(&events, id, JobState::Scanning);
                let plan = scan_sources(&*vfs, &sources, ScanOptions::default(), &control, |_| {})?;
                send_state(&events, id, JobState::Running);
                let mut emitter = ProgressEmitter::new(
                    id,
                    events.clone(),
                    (plan.total_bytes, sources.len() as u64),
                );
                let mut outcome = trash_sources(&*vfs, &sources, &control, |progress| {
                    emitter.advanced(progress.bytes, progress.items_finished, Some(progress.path));
                })?;
                outcome.errors.extend(plan.errors);
                emitter.flush();
                Ok(outcome)
            },
        )
    }

    #[must_use]
    pub fn spawn_delete_permanently(
        &self,
        sources: Vec<VPath>,
        confirmed_item_count: usize,
    ) -> JobHandle {
        let vfs = Arc::clone(&self.vfs);
        spawn(
            JobKind::DeletePermanent,
            self.journal_directory.clone(),
            sources.clone(),
            None,
            move |id, control, events, _| {
                send_state(&events, id, JobState::Scanning);
                let plan = scan_sources(&*vfs, &sources, ScanOptions::default(), &control, |_| {})?;
                send_state(&events, id, JobState::Running);
                let mut emitter = ProgressEmitter::new(
                    id,
                    events.clone(),
                    (plan.total_bytes, plan.entries.len() as u64),
                );
                let mut outcome = delete_permanently(
                    &*vfs,
                    &sources,
                    confirmed_item_count,
                    &control,
                    |progress| {
                        emitter.advanced(
                            progress.bytes,
                            progress.items_finished,
                            Some(progress.path),
                        );
                    },
                )?;
                outcome.errors.extend(plan.errors);
                emitter.flush();
                Ok(outcome)
            },
        )
    }
}

impl Default for OperationEngine {
    fn default() -> Self {
        Self::new(Arc::new(LocalFs))
    }
}

/// Controller and event stream for one active operation.
pub struct JobHandle {
    id: JobId,
    control: JobControl,
    events: Receiver<JobEvent>,
    decisions: Sender<ConflictResponse>,
    worker: Option<JoinHandle<JobSummary>>,
}

impl JobHandle {
    #[must_use]
    pub const fn id(&self) -> JobId {
        self.id
    }

    #[must_use]
    pub fn control(&self) -> JobControl {
        self.control.clone()
    }

    #[must_use]
    pub const fn events(&self) -> &Receiver<JobEvent> {
        &self.events
    }

    /// Responds to one conflict event.
    ///
    /// # Errors
    ///
    /// Returns the response when the worker has already exited.
    pub fn resolve_conflict(
        &self,
        conflict_id: ConflictId,
        decision: ConflictDecision,
    ) -> Result<(), ConflictResponse> {
        self.decisions
            .send(ConflictResponse {
                conflict_id,
                decision,
            })
            .map_err(|error| error.0)
    }

    pub fn pause(&self) {
        self.control.pause();
    }

    pub fn resume(&self) {
        self.control.resume();
    }

    pub fn cancel(&self) {
        self.control.cancel();
    }

    /// Waits for the worker and returns its complete summary.
    ///
    /// # Panics
    ///
    /// Panics if the operation worker panicked, which indicates an engine defect.
    #[must_use]
    pub fn join(mut self) -> JobSummary {
        self.worker
            .take()
            .expect("job worker already joined")
            .join()
            .expect("job worker panicked")
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.control.cancel();
            let _ = worker.join();
        }
    }
}

fn spawn(
    kind: JobKind,
    journal_directory: Option<std::path::PathBuf>,
    sources: Vec<VPath>,
    destination: Option<VPath>,
    run: impl FnOnce(
        JobId,
        JobControl,
        Sender<JobEvent>,
        Receiver<ConflictResponse>,
    ) -> Result<TransferOutcome, dualpane_core::Cancelled>
    + Send
    + 'static,
) -> JobHandle {
    let id = JobId::next();
    let control = JobControl::new();
    let (event_sender, events) = unbounded();
    let worker_control = control.clone().with_events(id, event_sender.clone());
    let (decisions, decision_receiver) = bounded(32);
    let worker = thread::Builder::new()
        .name(format!("dualpane-job-{}", id.get()))
        .spawn(move || {
            let journal = journal_directory
                .as_ref()
                .map(|directory| {
                    crate::journal::JobJournal::create(directory, kind, sources, destination)
                        .map(Arc::new)
                })
                .transpose();
            let (mut state, mut outcome, journal) = match journal {
                Ok(journal) => {
                    let control = journal.as_ref().map_or_else(
                        || worker_control.clone(),
                        |journal| worker_control.clone().with_journal(journal.clone()),
                    );
                    let result = run(id, control, event_sender.clone(), decision_receiver);
                    let (state, outcome) = match result {
                        Ok(outcome) => (
                            if outcome.errors.is_empty() {
                                JobState::Done
                            } else {
                                JobState::Failed
                            },
                            outcome,
                        ),
                        Err(_) => (
                            JobState::Cancelled,
                            journal
                                .as_ref()
                                .map_or_else(TransferOutcome::default, |journal| {
                                    journal.partial_outcome()
                                }),
                        ),
                    };
                    (state, outcome, journal)
                }
                Err(error) => (
                    JobState::Failed,
                    TransferOutcome {
                        errors: vec![error],
                        ..TransferOutcome::default()
                    },
                    None,
                ),
            };
            worker_control.phase(crate::JobPhase::Finishing, None);
            if let Some(journal) = journal {
                let partial = journal.partial_outcome();
                outcome.backups = partial.backups;
                outcome.journal_path = Some(journal.path().to_owned());
                // Final metadata (especially directory mtimes) supersedes earlier commits.
                for record in &outcome.transfers {
                    if let Err(error) = journal.transfer(record) {
                        state = JobState::Failed;
                        outcome.errors.push(error);
                        break;
                    }
                }
                if let Err(error) = journal.append(&crate::journal::JournalEvent::Finished {
                    state,
                    errors: outcome
                        .errors
                        .iter()
                        .map(|error| error.message.clone())
                        .collect(),
                    completed_items: outcome.completed_items,
                }) {
                    state = JobState::Failed;
                    outcome.errors.push(error);
                }
            }
            send_state(&event_sender, id, state);
            let _ = event_sender.send(JobEvent::Finished {
                id,
                state,
                errors: outcome.errors.clone(),
            });
            JobSummary {
                id,
                kind,
                state,
                outcome,
            }
        })
        .expect("failed to spawn operation worker");
    JobHandle {
        id,
        control,
        events,
        decisions,
        worker: Some(worker),
    }
}

fn await_conflict(
    id: JobId,
    conflict: &Conflict,
    control: &JobControl,
    events: &Sender<JobEvent>,
    decisions: &Receiver<ConflictResponse>,
) -> Result<ConflictDecision, dualpane_core::Cancelled> {
    let conflict_id = ConflictId::next();
    let _ = events.send(JobEvent::Conflict {
        id,
        conflict_id,
        conflict: Box::new(conflict.clone()),
    });
    loop {
        control.checkpoint()?;
        match decisions.recv_timeout(Duration::from_millis(50)) {
            Ok(response) if response.conflict_id == conflict_id => return Ok(response.decision),
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(dualpane_core::Cancelled);
            }
        }
    }
}

fn send_state(events: &Sender<JobEvent>, id: JobId, state: JobState) {
    let _ = events.send(JobEvent::State { id, state });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::Arc;
    use std::time::Duration;

    use dualpane_core::{CancelToken, Capabilities, Entry, EntryKind, Metadata, SizeHint, VPath};
    use dualpane_vfs::{LocalFs, ReadSeek, Result as VfsResult, Vfs, WriteSeek};
    use tempfile::tempdir;

    use super::OperationEngine;
    use crate::{
        ConflictAction, ConflictDecision, JobEvent, JobState, ScanOptions, TransferOptions,
    };

    #[test]
    fn independent_jobs_run_concurrently_and_finish() {
        let fixture = tempdir().expect("fixture");
        let destination = fixture.path().join("destination");
        fs::create_dir(&destination).expect("destination");
        let one = fixture.path().join("one");
        let two = fixture.path().join("two");
        fs::write(&one, vec![1_u8; 1024 * 1024]).expect("one");
        fs::write(&two, vec![2_u8; 1024 * 1024]).expect("two");
        let engine = OperationEngine::default();

        let first = engine.spawn_copy(
            vec![VPath::from(one.as_path())],
            VPath::from(destination.as_path()),
            ScanOptions::default(),
            TransferOptions::default(),
        );
        let second = engine.spawn_copy(
            vec![VPath::from(two.as_path())],
            VPath::from(destination.as_path()),
            ScanOptions::default(),
            TransferOptions::default(),
        );

        assert_eq!(first.join().state, JobState::Done);
        assert_eq!(second.join().state, JobState::Done);
        assert!(destination.join("one").exists());
        assert!(destination.join("two").exists());
    }

    #[test]
    fn ask_conflict_does_not_block_non_conflicting_items() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        fs::create_dir(&source).expect("source");
        fs::create_dir(&destination).expect("destination");
        fs::create_dir(destination.join("source")).expect("destination root");
        fs::write(source.join("a-conflict"), b"new").expect("conflict source");
        fs::write(source.join("z-unrelated"), b"continues").expect("unrelated source");
        fs::write(destination.join("source/a-conflict"), b"old").expect("conflict target");
        let handle = OperationEngine::default().spawn_copy(
            vec![VPath::from(source.as_path())],
            VPath::from(destination.as_path()),
            ScanOptions::default(),
            TransferOptions::default(),
        );

        let conflict_id = loop {
            match handle
                .events()
                .recv_timeout(Duration::from_secs(5))
                .expect("job event")
            {
                JobEvent::Conflict { conflict_id, .. } => break conflict_id,
                JobEvent::Phase { .. }
                | JobEvent::State { .. }
                | JobEvent::Progress { .. }
                | JobEvent::Finished { .. } => {}
            }
        };
        assert_eq!(
            fs::read(destination.join("source/z-unrelated")).expect("unrelated copied first"),
            b"continues"
        );
        handle
            .resolve_conflict(
                conflict_id,
                ConflictDecision {
                    action: ConflictAction::Skip,
                    apply_to_all: false,
                },
            )
            .expect("resolve");
        let summary = handle.join();

        assert_eq!(summary.state, JobState::Done);
        assert_eq!(
            fs::read(destination.join("source/a-conflict")).expect("conflict kept"),
            b"old"
        );
    }

    #[test]
    fn cancelling_mid_copy_removes_the_atomic_temporary_file() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("large.bin");
        let destination = fixture.path().join("destination");
        fs::write(&source, vec![0x5a_u8; 32 * 1024 * 1024]).expect("source");
        fs::create_dir(&destination).expect("destination");
        let handle = OperationEngine::new(Arc::new(SlowFs {
            no_space: false,
            range_gate: None,
        }))
        .spawn_copy(
            vec![VPath::from(source.as_path())],
            VPath::from(destination.as_path()),
            ScanOptions::default(),
            TransferOptions::default(),
        );

        loop {
            match handle
                .events()
                .recv_timeout(Duration::from_secs(5))
                .expect("progress before timeout")
            {
                JobEvent::Progress { progress, .. } if progress.bytes_done > 0 => break,
                JobEvent::Phase { .. }
                | JobEvent::State { .. }
                | JobEvent::Progress { .. }
                | JobEvent::Conflict { .. }
                | JobEvent::Finished { .. } => {}
            }
        }
        handle.cancel();
        let summary = handle.join();

        assert_eq!(summary.state, JobState::Cancelled);
        assert_eq!(fs::read_dir(destination).expect("destination").count(), 0);
    }

    #[test]
    fn cancelling_a_staged_tree_removes_the_hidden_root() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source-tree");
        let destination = fixture.path().join("destination");
        fs::create_dir(&source).expect("source tree");
        fs::create_dir(&destination).expect("destination");
        fs::write(source.join("large.bin"), vec![0x5a_u8; 32 * 1024 * 1024]).expect("source file");
        let handle = OperationEngine::new(Arc::new(SlowFs {
            no_space: false,
            range_gate: None,
        }))
        .spawn_copy(
            vec![VPath::from(source.as_path())],
            VPath::from(destination.as_path()),
            ScanOptions::default(),
            TransferOptions::default(),
        );

        loop {
            match handle
                .events()
                .recv_timeout(Duration::from_secs(5))
                .expect("progress before timeout")
            {
                JobEvent::Progress { progress, .. } if progress.bytes_done > 0 => break,
                JobEvent::Phase { .. }
                | JobEvent::State { .. }
                | JobEvent::Progress { .. }
                | JobEvent::Conflict { .. }
                | JobEvent::Finished { .. } => {}
            }
        }
        handle.cancel();
        let summary = handle.join();

        assert_eq!(summary.state, JobState::Cancelled);
        assert_eq!(fs::read_dir(destination).expect("destination").count(), 0);
    }

    #[test]
    fn cross_device_move_verifies_before_removing_the_source() {
        let source_parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        fs::create_dir_all(&source_parent).expect("source fixture parent");
        let source_root = tempfile::tempdir_in(source_parent).expect("source fixture");
        let destination = tempdir().expect("destination fixture");
        let source = source_root.path().join("move-me.bin");
        fs::write(&source, b"cross-device payload").expect("source");
        let source_device = LocalFs
            .stat(&VPath::from(source.as_path()), false)
            .expect("source metadata")
            .identity
            .expect("source identity")
            .device;
        let destination_device = LocalFs
            .stat(&VPath::from(destination.path()), false)
            .expect("destination metadata")
            .identity
            .expect("destination identity")
            .device;
        if source_device == destination_device {
            return;
        }

        let summary = OperationEngine::default()
            .spawn_move(
                vec![VPath::from(source.as_path())],
                VPath::from(destination.path()),
                ScanOptions::default(),
                TransferOptions::default(),
            )
            .join();

        assert_eq!(summary.state, JobState::Done);
        assert!(
            summary.outcome.errors.is_empty(),
            "{:?}",
            summary.outcome.errors
        );
        assert!(!source.exists());
        assert_eq!(
            fs::read(destination.path().join("move-me.bin")).expect("moved payload"),
            b"cross-device payload"
        );
    }

    #[test]
    fn insufficient_destination_space_pauses_until_cancelled_or_resumed() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source.bin");
        let destination = fixture.path().join("destination");
        fs::write(&source, b"payload").expect("source");
        fs::create_dir(&destination).expect("destination");
        let handle = OperationEngine::new(Arc::new(SlowFs {
            no_space: true,
            range_gate: None,
        }))
        .spawn_copy(
            vec![VPath::from(source.as_path())],
            VPath::from(destination.as_path()),
            ScanOptions::default(),
            TransferOptions::default(),
        );

        loop {
            match handle
                .events()
                .recv_timeout(Duration::from_secs(5))
                .expect("paused state")
            {
                JobEvent::State {
                    state: JobState::Paused,
                    ..
                } => break,
                JobEvent::Phase { .. }
                | JobEvent::State { .. }
                | JobEvent::Progress { .. }
                | JobEvent::Conflict { .. }
                | JobEvent::Finished { .. } => {}
            }
        }
        handle.cancel();

        assert_eq!(handle.join().state, JobState::Cancelled);
        assert_eq!(fs::read_dir(destination).expect("destination").count(), 0);
    }

    #[test]
    fn verification_observes_pause_and_cancellation() {
        for cancel in [false, true] {
            let fixture = tempdir().unwrap();
            let source = VPath::from(fixture.path().join("source"));
            let destination = VPath::from(fixture.path().join("destination"));
            fs::write(source.as_path(), vec![7_u8; 4 * 1024 * 1024]).unwrap();
            fs::copy(source.as_path(), destination.as_path()).unwrap();
            let metadata = LocalFs.stat(&source, false).unwrap();
            LocalFs
                .preserve_metadata(&source, &destination, &metadata)
                .unwrap();
            let control = crate::JobControl::new();
            control.pause();
            let worker_control = control.clone();
            let (send, receive) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let _ = send.send(crate::verify_copy_controlled(
                    &LocalFs,
                    &source,
                    &destination,
                    &worker_control,
                ));
            });
            assert!(
                receive.recv_timeout(Duration::from_millis(60)).is_err(),
                "verification must wait at the pause gate"
            );
            if cancel {
                control.cancel();
            } else {
                control.resume();
            }
            assert_eq!(
                receive
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .is_err(),
                cancel
            );
            worker.join().unwrap();
        }
    }

    #[test]
    fn optimized_copy_stops_between_range_chunks() {
        let fixture = tempdir().unwrap();
        let source = VPath::from(fixture.path().join("source"));
        let destination = VPath::from(fixture.path().join("destination"));
        fs::write(source.as_path(), vec![9_u8; 1024 * 1024]).unwrap();
        fs::create_dir(destination.as_path()).unwrap();
        let control = crate::JobControl::new();
        let vfs = SlowFs {
            no_space: false,
            range_gate: Some(control.clone()),
        };
        let plan = crate::scan_sources(
            &LocalFs,
            std::slice::from_ref(&source),
            ScanOptions::default(),
            &control,
            |_| {},
        )
        .unwrap();
        let worker_control = control.clone();
        let worker_destination = destination.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = crate::copy_plan(
                &vfs,
                &plan,
                &worker_destination,
                TransferOptions {
                    parallel: false,
                    ..TransferOptions::default()
                },
                &worker_control,
                |_| {},
                |_| {},
                |_| Err(dualpane_core::Cancelled),
            );
            let _ = send.send(result);
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !control.is_paused() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(receive.recv_timeout(Duration::from_millis(60)).is_err());
        let temporary = fs::read_dir(destination.as_path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::metadata(temporary).unwrap().len(), 64 * 1024);
        control.resume();
        assert!(
            receive
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap()
                .errors
                .is_empty()
        );
        worker.join().unwrap();
        assert_eq!(
            fs::read(destination.as_path().join("source")).unwrap(),
            fs::read(source.as_path()).unwrap()
        );
    }

    #[test]
    fn cancelled_journalled_copy_keeps_its_committed_summary() {
        let fixture = tempdir().unwrap();
        let destination = fixture.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let small = fixture.path().join("a-small");
        let large = fixture.path().join("b-large");
        fs::write(&small, b"complete").unwrap();
        fs::write(&large, vec![3_u8; 32 * 1024 * 1024]).unwrap();
        let engine = OperationEngine::new(Arc::new(SlowFs {
            no_space: false,
            range_gate: None,
        }))
        .with_journal_directory(fixture.path().join("jobs"));
        let handle = engine.spawn_copy(
            vec![VPath::from(small.as_path()), VPath::from(large.as_path())],
            VPath::from(destination.as_path()),
            ScanOptions::default(),
            TransferOptions {
                parallel: false,
                ..TransferOptions::default()
            },
        );
        loop {
            let event = handle
                .events()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            if matches!(event, JobEvent::Progress { progress, .. } if progress.bytes_done > 1024 * 1024)
            {
                break;
            }
        }
        handle.cancel();
        let result = handle.join();
        assert_eq!(result.state, JobState::Cancelled);
        assert!(
            result
                .outcome
                .transfers
                .iter()
                .any(|record| record.destination == VPath::from(destination.join("a-small")))
        );
        let record =
            crate::journal::read_journal(result.outcome.journal_path.as_ref().unwrap()).unwrap();
        assert_eq!(record.state, Some(JobState::Cancelled));
        assert_eq!(record.transfers.len(), 1);
        assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
    }

    struct SlowFs {
        no_space: bool,
        range_gate: Option<crate::JobControl>,
    }

    impl Vfs for SlowFs {
        fn read_dir(
            &self,
            path: &VPath,
            cancel: &CancelToken,
        ) -> VfsResult<Box<dyn Iterator<Item = VfsResult<Entry>> + Send>> {
            LocalFs.read_dir(path, cancel)
        }

        fn stat(&self, path: &VPath, follow: bool) -> VfsResult<Metadata> {
            LocalFs.stat(path, follow)
        }

        fn open_read(&self, path: &VPath) -> VfsResult<Box<dyn ReadSeek>> {
            LocalFs
                .open_read(path)
                .map(|inner| Box::new(SlowReader(inner)) as Box<dyn ReadSeek>)
        }

        fn create_write(&self, path: &VPath, hint: SizeHint) -> VfsResult<Box<dyn WriteSeek>> {
            LocalFs.create_write(path, hint)
        }

        fn open_write(&self, path: &VPath) -> VfsResult<Box<dyn WriteSeek>> {
            LocalFs.open_write(path)
        }

        fn create_dir(&self, path: &VPath) -> VfsResult<()> {
            LocalFs.create_dir(path)
        }

        fn reflink(&self, _source: &VPath, _destination: &VPath) -> VfsResult<bool> {
            Ok(false)
        }

        fn copy_file_range(
            &self,
            source: &VPath,
            destination: &VPath,
            offset: u64,
            length: u64,
            cancel: &CancelToken,
            progress: &mut dyn FnMut(u64),
        ) -> VfsResult<u64> {
            let Some(gate) = &self.range_gate else {
                return Err(dualpane_vfs::VfsError::Unsupported {
                    operation: "test range copy",
                    path: destination.clone(),
                });
            };
            use std::io::Write;
            let mut reader = LocalFs.open_read(source)?;
            let mut writer = LocalFs.open_write(destination)?;
            reader.seek(SeekFrom::Start(offset)).unwrap();
            writer.seek(SeekFrom::Start(offset)).unwrap();
            let mut done = 0;
            let mut bytes = vec![0_u8; 64 * 1024];
            while done < length {
                cancel
                    .check()
                    .map_err(|_| dualpane_vfs::VfsError::Cancelled)?;
                let count = (length - done).min(bytes.len() as u64) as usize;
                reader.read_exact(&mut bytes[..count]).unwrap();
                writer.write_all(&bytes[..count]).unwrap();
                if done == 0 {
                    gate.pause();
                }
                progress(count as u64);
                done += count as u64;
            }
            Ok(done)
        }

        fn sparse_ranges(&self, _path: &VPath, _length: u64) -> VfsResult<Option<Vec<(u64, u64)>>> {
            Ok(None)
        }

        fn set_len(&self, path: &VPath, length: u64) -> VfsResult<()> {
            LocalFs.set_len(path, length)
        }

        fn preserve_metadata(
            &self,
            source: &VPath,
            destination: &VPath,
            metadata: &Metadata,
        ) -> VfsResult<Vec<String>> {
            LocalFs.preserve_metadata(source, destination, metadata)
        }

        fn available_space(&self, path: &VPath) -> VfsResult<Option<u64>> {
            if self.no_space {
                Ok(Some(0))
            } else {
                LocalFs.available_space(path)
            }
        }

        fn rename_noreplace(&self, from: &VPath, to: &VPath) -> VfsResult<()> {
            LocalFs.rename_noreplace(from, to)
        }

        fn rename(&self, from: &VPath, to: &VPath) -> VfsResult<()> {
            LocalFs.rename(from, to)
        }

        fn remove(&self, path: &VPath, kind: EntryKind) -> VfsResult<()> {
            LocalFs.remove(path, kind)
        }

        fn capabilities(&self) -> Capabilities {
            LocalFs.capabilities()
        }
    }

    struct SlowReader(Box<dyn ReadSeek>);

    impl Read for SlowReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(Duration::from_millis(2));
            self.0.read(buffer)
        }
    }

    impl Seek for SlowReader {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.0.seek(position)
        }
    }
}
