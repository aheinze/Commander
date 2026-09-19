use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use dualpane_core::{CancelToken, VPath};

use crate::JobError;
use crate::{Conflict, ConflictDecision, TransferOutcome};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
const EWMA_WINDOW_SECONDS: f64 = 5.0;
static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CONFLICT_ID: AtomicU64 = AtomicU64::new(1);

/// Process-local stable identifier for a file-operation job.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct JobId(u64);

impl JobId {
    #[must_use]
    pub fn next() -> Self {
        Self(NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Stable identifier for one queued conflict request.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConflictId(u64);

impl ConflictId {
    #[must_use]
    pub fn next() -> Self {
        Self(NEXT_CONFLICT_ID.fetch_add(1, Ordering::Relaxed))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// User-visible operation category.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobKind {
    Copy,
    Move,
    Trash,
    DeletePermanent,
}

/// Explicit job state machine.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    Scanning,
    Running,
    Paused,
    Done,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum JobPhase {
    #[default]
    Preparing,
    Copying,
    Verifying,
    Finishing,
}

/// Coalesced job progress, published at no more than 20 Hz.
#[derive(Clone, Debug, PartialEq)]
pub struct JobProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub items_done: u64,
    pub items_total: u64,
    pub current_path: Option<VPath>,
    pub throughput_bytes_per_second: f64,
    pub eta: Option<Duration>,
}

impl Default for JobProgress {
    fn default() -> Self {
        Self {
            bytes_done: 0,
            bytes_total: 0,
            items_done: 0,
            items_total: 0,
            current_path: None,
            throughput_bytes_per_second: 0.0,
            eta: None,
        }
    }
}

/// A destination-space estimate that requires a user decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpaceIssue {
    pub destination: VPath,
    pub required_bytes: u64,
    pub available_bytes: Option<u64>,
    pub error: Option<String>,
    pub checking: bool,
}

#[derive(Debug, Default)]
struct PauseGate {
    paused: bool,
    space: Option<SpaceIssue>,
    recheck: bool,
}

/// Events consumed by the app or a future task centre.
#[derive(Clone, Debug, PartialEq)]
pub enum JobEvent {
    SpaceChanged {
        id: JobId,
    },
    Phase {
        id: JobId,
        phase: JobPhase,
        path: Option<VPath>,
    },
    State {
        id: JobId,
        state: JobState,
    },
    Progress {
        id: JobId,
        progress: JobProgress,
    },
    Conflict {
        id: JobId,
        conflict_id: ConflictId,
        conflict: Box<Conflict>,
    },
    Finished {
        id: JobId,
        state: JobState,
        errors: Vec<JobError>,
    },
}

/// Response sent back to a running job for one conflict request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConflictResponse {
    pub conflict_id: ConflictId,
    pub decision: ConflictDecision,
}

/// Final headless result returned when joining a job worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSummary {
    pub id: JobId,
    pub kind: JobKind,
    pub state: JobState,
    pub outcome: TransferOutcome,
}

/// Shared cancellation and pause gate checked by scan and transfer loops.
#[derive(Clone, Debug)]
pub struct JobControl {
    cancel: CancelToken,
    pause: Arc<(Mutex<PauseGate>, Condvar)>,
    journal: Option<Arc<crate::journal::JobJournal>>,
    phase_sink: Option<(JobId, Sender<JobEvent>)>,
    last_phase_emit: Arc<Mutex<Option<Instant>>>,
    retry_records: Arc<std::collections::HashMap<VPath, crate::TransferRecord>>,
}

impl JobControl {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancel: CancelToken::new(),
            pause: Arc::new((Mutex::new(PauseGate::default()), Condvar::new())),
            journal: None,
            phase_sink: None,
            last_phase_emit: Arc::new(Mutex::new(None)),
            retry_records: Arc::default(),
        }
    }

    #[must_use]
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    pub(crate) fn with_retry_records(mut self, records: Vec<crate::TransferRecord>) -> Self {
        self.retry_records = Arc::new(
            records
                .into_iter()
                .map(|record| (record.source.clone(), record))
                .collect(),
        );
        self
    }

    pub(crate) fn retry_record(&self, source: &VPath) -> Option<&crate::TransferRecord> {
        self.retry_records.get(source)
    }

    pub(crate) fn retry_records(&self) -> impl Iterator<Item = &crate::TransferRecord> {
        self.retry_records.values()
    }

    pub(crate) fn is_retry(&self) -> bool {
        !self.retry_records.is_empty()
    }

    pub(crate) fn with_journal(mut self, journal: Arc<crate::journal::JobJournal>) -> Self {
        self.journal = Some(journal);
        self
    }
    pub(crate) fn with_events(mut self, id: JobId, events: Sender<JobEvent>) -> Self {
        self.phase_sink = Some((id, events));
        self
    }
    pub(crate) fn intent(
        &self,
        action: &str,
        source: Option<&VPath>,
        target: &VPath,
    ) -> Result<Option<u64>, JobError> {
        self.journal
            .as_ref()
            .map(|journal| journal.intent(action, source, target))
            .transpose()
    }
    pub(crate) fn applied(&self, sequence: Option<u64>) -> Result<(), JobError> {
        if let Some((journal, sequence)) = self.journal.as_ref().zip(sequence) {
            journal.applied(sequence)?;
        }
        Ok(())
    }
    pub fn journal(&self) -> Option<&Arc<crate::journal::JobJournal>> {
        self.journal.as_ref()
    }
    pub fn phase(&self, phase: JobPhase, path: Option<VPath>) {
        if let Some((id, events)) = &self.phase_sink {
            let mut last = self.last_phase_emit.lock().expect("phase mutex poisoned");
            if last.is_none_or(|last| last.elapsed() >= PROGRESS_INTERVAL) {
                *last = Some(Instant::now());
                let _ = events.send(JobEvent::Phase {
                    id: *id,
                    phase,
                    path,
                });
            }
        }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
        self.resume();
    }

    pub fn pause(&self) {
        let (paused, _) = &*self.pause;
        paused.lock().expect("pause mutex poisoned").paused = true;
    }

    /// Whether the worker's pause gate is currently closed.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.pause.0.lock().expect("pause mutex poisoned").paused
    }

    pub fn resume(&self) {
        let (paused, wake) = &*self.pause;
        let mut gate = paused.lock().expect("pause mutex poisoned");
        gate.paused = false;
        let space = gate.space.take().is_some();
        gate.recheck = false;
        drop(gate);
        wake.notify_all();
        if space {
            self.space_changed();
        }
    }

    #[must_use]
    pub fn space_issue(&self) -> Option<SpaceIssue> {
        self.pause
            .0
            .lock()
            .expect("pause mutex poisoned")
            .space
            .clone()
    }

    /// Ask the job worker to check free space again; no filesystem I/O runs here.
    pub fn recheck_space(&self) {
        let mut gate = self.pause.0.lock().expect("pause mutex poisoned");
        if let Some(space) = &mut gate.space
            && !space.checking
        {
            space.checking = true;
            gate.recheck = true;
            drop(gate);
            self.pause.1.notify_all();
            self.space_changed();
        }
    }

    fn space_changed(&self) {
        if let Some((id, events)) = &self.phase_sink {
            let _ = events.send(JobEvent::SpaceChanged { id: *id });
        }
    }

    /// Wait for a recheck or an explicit Resume override of a low-space estimate.
    pub(crate) fn wait_for_space(
        &self,
        destination: &VPath,
        required_bytes: u64,
        available_bytes: u64,
        mut query: impl FnMut() -> Result<Option<u64>, String>,
        on_paused: impl FnOnce(),
    ) -> Result<(), dualpane_core::Cancelled> {
        let (paused, wake) = &*self.pause;
        {
            let mut gate = paused.lock().expect("pause mutex poisoned");
            gate.paused = true;
            gate.recheck = false;
            gate.space = Some(SpaceIssue {
                destination: destination.clone(),
                required_bytes,
                available_bytes: Some(available_bytes),
                error: None,
                checking: false,
            });
        }
        on_paused();
        self.space_changed();
        loop {
            let mut gate = paused.lock().expect("pause mutex poisoned");
            while gate.space.is_some()
                && gate.paused
                && !gate.recheck
                && !self.cancel.is_cancelled()
            {
                gate = wake.wait(gate).expect("pause mutex poisoned");
            }
            self.cancel.check()?;
            if gate.space.is_none() || !gate.paused {
                return Ok(());
            }
            gate.recheck = false;
            drop(gate);
            let result = query();
            let mut gate = paused.lock().expect("pause mutex poisoned");
            // A cancelled/resumed check cannot re-pause the job when a slow query returns.
            self.cancel.check()?;
            if gate.space.is_none() {
                return Ok(());
            }
            match result {
                Ok(Some(available)) if available >= required_bytes => {
                    gate.space = None;
                    gate.paused = false;
                    drop(gate);
                    wake.notify_all();
                    self.space_changed();
                    return Ok(());
                }
                result => {
                    let space = gate.space.as_mut().unwrap();
                    space.checking = false;
                    let (available, error) = match result {
                        Ok(Some(bytes)) => (Some(bytes), None),
                        Ok(None) => (
                            None,
                            Some("The destination does not report available space.".into()),
                        ),
                        Err(error) => (None, Some(format!("Could not check free space: {error}"))),
                    };
                    space.available_bytes = available;
                    space.error = error;
                }
            }
            drop(gate);
            self.space_changed();
        }
    }

    /// Waits while paused and then observes cancellation.
    ///
    /// # Errors
    ///
    /// Returns cancellation after a cancel request, including one made while paused.
    pub fn checkpoint(&self) -> Result<(), dualpane_core::Cancelled> {
        let (paused, wake) = &*self.pause;
        let mut guard = paused.lock().expect("pause mutex poisoned");
        while guard.paused && !self.cancel.is_cancelled() {
            guard = wake.wait(guard).expect("pause mutex poisoned");
        }
        drop(guard);
        self.cancel.check()
    }
}

impl Default for JobControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Progress coalescer with a five-second EWMA throughput estimate.
pub struct ProgressEmitter {
    id: JobId,
    sender: Sender<JobEvent>,
    progress: JobProgress,
    last_emit: Instant,
    last_sample: Instant,
    last_sample_bytes: u64,
}

impl ProgressEmitter {
    #[must_use]
    pub fn new(id: JobId, sender: Sender<JobEvent>, totals: (u64, u64)) -> Self {
        let now = Instant::now();
        Self {
            id,
            sender,
            progress: JobProgress {
                bytes_total: totals.0,
                items_total: totals.1,
                ..JobProgress::default()
            },
            last_emit: now.checked_sub(PROGRESS_INTERVAL).unwrap_or(now),
            last_sample: now,
            last_sample_bytes: 0,
        }
    }

    pub fn advanced(&mut self, bytes: u64, items: u64, current_path: Option<VPath>) {
        self.progress.bytes_done = self.progress.bytes_done.saturating_add(bytes);
        self.progress.items_done = self.progress.items_done.saturating_add(items);
        self.progress.current_path = current_path;
        self.emit(false);
    }

    pub fn flush(&mut self) {
        self.emit(true);
    }

    fn update_rate(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_sample).as_secs_f64();
        if elapsed <= 0.0 {
            return;
        }
        let bytes = self
            .progress
            .bytes_done
            .saturating_sub(self.last_sample_bytes);
        let instantaneous = bytes as f64 / elapsed;
        let alpha = 1.0 - (-elapsed / EWMA_WINDOW_SECONDS).exp();
        self.progress.throughput_bytes_per_second =
            if self.progress.throughput_bytes_per_second == 0.0 {
                instantaneous
            } else {
                alpha.mul_add(
                    instantaneous,
                    (1.0 - alpha) * self.progress.throughput_bytes_per_second,
                )
            };
        let remaining = self
            .progress
            .bytes_total
            .saturating_sub(self.progress.bytes_done);
        self.progress.eta = (self.progress.throughput_bytes_per_second > 0.0).then(|| {
            Duration::from_secs_f64(remaining as f64 / self.progress.throughput_bytes_per_second)
        });
        self.last_sample = now;
        self.last_sample_bytes = self.progress.bytes_done;
    }

    fn emit(&mut self, force: bool) {
        let now = Instant::now();
        if force || now.duration_since(self.last_emit) >= PROGRESS_INTERVAL {
            self.update_rate();
            let _ = self.sender.send(JobEvent::Progress {
                id: self.id,
                progress: self.progress.clone(),
            });
            self.last_emit = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use crossbeam_channel::unbounded;

    use super::{JobControl, JobEvent, JobId, ProgressEmitter};

    #[test]
    fn cancelling_a_paused_control_wakes_the_waiter() {
        let control = JobControl::new();
        control.pause();
        let waiter = control.clone();
        let worker = thread::spawn(move || waiter.checkpoint());

        control.cancel();

        assert!(worker.join().expect("worker").is_err());
    }

    #[test]
    fn progress_is_coalesced_and_flushes_the_final_value() {
        let (sender, receiver) = unbounded();
        let mut emitter = ProgressEmitter::new(JobId::next(), sender, (100, 10));
        for _ in 0..10 {
            emitter.advanced(10, 1, None);
        }
        emitter.flush();
        thread::sleep(Duration::from_millis(1));
        let events: Vec<_> = receiver.try_iter().collect();

        assert!(events.len() <= 2);
        let JobEvent::Progress { progress, .. } = events.last().expect("progress") else {
            panic!("unexpected event");
        };
        assert_eq!(progress.bytes_done, 100);
        assert_eq!(progress.items_done, 10);
    }
}

#[cfg(test)]
#[path = "job_space_tests.rs"]
mod space_tests;
