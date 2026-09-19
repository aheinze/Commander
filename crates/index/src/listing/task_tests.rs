use super::*;
use crossbeam_channel::bounded;
use dualpane_core::{Capabilities, EntryKind, SizeHint};
use dualpane_vfs::{LocalFs, ReadSeek, Result as VfsResult, WriteSeek};

struct BlockedFs {
    opening: bool,
    entered: Sender<()>,
    release: Receiver<()>,
    exited: Sender<()>,
}

impl Drop for BlockedFs {
    fn drop(&mut self) {
        let _ = self.exited.send(());
    }
}

impl Vfs for BlockedFs {
    fn read_dir(
        &self,
        _: &VPath,
        _: &CancelToken,
    ) -> VfsResult<Box<dyn Iterator<Item = VfsResult<Entry>> + Send>> {
        if self.opening {
            self.entered.send(()).unwrap();
            let _ = self.release.recv();
        }
        let opening = self.opening;
        let entered = self.entered.clone();
        let release = self.release.clone();
        Ok(Box::new(std::iter::once_with(move || {
            if !opening {
                entered.send(()).unwrap();
                let _ = release.recv();
            }
            Ok(Entry::new("remote-file".into(), EntryKind::File, None))
        })))
    }

    fn stat(&self, path: &VPath, follow: bool) -> VfsResult<Metadata> {
        LocalFs.stat(path, follow)
    }

    fn open_read(&self, path: &VPath) -> VfsResult<Box<dyn ReadSeek>> {
        LocalFs.open_read(path)
    }

    fn create_write(&self, path: &VPath, hint: SizeHint) -> VfsResult<Box<dyn WriteSeek>> {
        LocalFs.create_write(path, hint)
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

fn blocked_task(opening: bool, timeout: Duration) -> (ListingTask, Sender<()>, Receiver<()>) {
    let (entered, ready) = bounded(1);
    let (release, blocked) = bounded(1);
    let (exited, finished) = bounded(1);
    let task = ListingTask::spawn_with_timeout(
        Arc::new(BlockedFs {
            opening,
            entered,
            release: blocked,
            exited,
        }),
        ListingRequest::new(VPath::from("stalled-remote")),
        timeout,
    )
    .unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    (task, release, finished)
}

#[test]
fn stalled_open_and_enumeration_time_out_without_waiting_for_io() {
    for opening in [true, false] {
        let (task, release, finished) = blocked_task(opening, Duration::from_millis(50));
        assert!(matches!(
            task.wait_complete(),
            Err(IndexError::ListingTimeout { path }) if path == VPath::from("stalled-remote")
        ));
        assert!(task.cancel_token().is_cancelled());
        let (dropped, received) = bounded(1);
        let worker = thread::spawn(move || {
            drop(task);
            dropped.send(()).unwrap();
        });
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        // The stalled syscall is still outstanding when the task owner returns.
        assert!(finished.try_recv().is_err());
        release.send(()).unwrap();
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }
}

#[test]
fn cancellation_interrupts_a_stalled_listing_before_the_timeout() {
    for external in [false, true] {
        let (task, release, finished) = blocked_task(true, Duration::from_secs(30));
        let cancel = CancelToken::new();
        let cancel_worker = cancel.clone();
        let own_cancel = task.cancel_token();
        let (result, received) = bounded(1);
        let waiter = thread::spawn(move || {
            result
                .send(task.next_event_cancellable(&cancel_worker))
                .unwrap();
        });
        if external {
            cancel.cancel();
        } else {
            own_cancel.cancel();
        }
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err(IndexError::Cancelled)
        ));
        waiter.join().unwrap();
        release.send(()).unwrap();
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
    }
}

#[test]
fn progress_resets_the_inactivity_timeout() {
    let timeout = Duration::from_millis(200);
    let task = ListingTask::spawn_with_timeout(
        Arc::new(super::tests::SlowVfs),
        ListingRequest::new(VPath::from("slow-but-responsive")),
        timeout,
    )
    .unwrap();
    let started = Instant::now();
    while started.elapsed() < timeout * 2 {
        assert!(matches!(
            task.next_event().unwrap(),
            ListingEvent::Snapshot { .. }
        ));
    }
    task.cancel();
    task.join().unwrap();
}

#[test]
fn a_timed_out_listing_cannot_later_report_success() {
    let (task, release, finished) = blocked_task(true, Duration::from_millis(50));
    assert!(matches!(
        task.next_event(),
        Err(IndexError::ListingTimeout { .. })
    ));
    release.send(()).unwrap();
    finished.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(task.next_event(), Err(IndexError::Cancelled)));
    task.join().unwrap();
}
