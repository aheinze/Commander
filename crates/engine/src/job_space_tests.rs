use super::*;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::thread;

type SpaceResult = Result<Option<u64>, String>;
struct Waiting {
    control: JobControl,
    events: Receiver<JobEvent>,
    queries: Receiver<()>,
    replies: Sender<SpaceResult>,
    done: Receiver<Result<(), dualpane_core::Cancelled>>,
}
impl Waiting {
    fn new() -> Self {
        let (events, receive) = unbounded();
        let control = JobControl::new().with_events(JobId::next(), events);
        let worker = control.clone();
        let (query, queries) = bounded(4);
        let (replies, reply) = bounded(4);
        let (finish, done) = bounded(1);
        thread::spawn(move || {
            let result = worker.wait_for_space(
                &"/destination".into(),
                100,
                10,
                || {
                    query.send(()).unwrap();
                    reply.recv_timeout(Duration::from_secs(5)).unwrap()
                },
                || {
                    // The Paused callback may immediately be handled by the UI.
                    assert!(worker.is_paused());
                    assert_eq!(worker.space_issue().unwrap().required_bytes, 100);
                },
            );
            let _ = finish.send(result);
        });
        let result = Self {
            control,
            events: receive,
            queries,
            replies,
            done,
        };
        result.wait(|| result.control.space_issue().is_some());
        result
    }
    fn wait(&self, mut condition: impl FnMut() -> bool) {
        while !condition() {
            self.events.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
    fn recheck(&self, result: SpaceResult) {
        self.control.recheck_space();
        self.queries.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(self.control.space_issue().unwrap().checking);
        self.replies.send(result).unwrap();
        self.wait(|| {
            self.control
                .space_issue()
                .is_none_or(|space| !space.checking)
        });
    }
    fn finished(&self, cancelled: bool) {
        assert_eq!(
            self.done
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .is_err(),
            cancelled
        );
        assert!(!self.control.is_paused());
        assert!(self.control.space_issue().is_none());
    }
}
impl Drop for Waiting {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

#[test]
fn low_space_rechecks_stay_paused_until_enough_space_is_available() {
    let waiting = Waiting::new();
    let reason = waiting.control.space_issue().unwrap();
    assert_eq!(reason.destination, "/destination".into());
    assert_eq!(reason.available_bytes, Some(10));
    waiting.recheck(Ok(Some(40)));
    assert!(waiting.control.is_paused());
    assert_eq!(
        waiting.control.space_issue().unwrap().available_bytes,
        Some(40)
    );
    assert!(waiting.done.try_recv().is_err());
    waiting.recheck(Ok(Some(100)));
    waiting.finished(false);
}

#[test]
fn unsupported_and_failed_space_rechecks_keep_controls_and_explain_the_failure() {
    let waiting = Waiting::new();
    waiting.recheck(Ok(None));
    assert!(waiting.control.is_paused());
    let issue = waiting.control.space_issue().unwrap();
    assert_eq!(issue.available_bytes, None);
    assert!(issue.error.unwrap().contains("does not report"));
    waiting.recheck(Err("connection lost".into()));
    assert!(waiting.control.is_paused());
    assert!(
        waiting
            .control
            .space_issue()
            .unwrap()
            .error
            .unwrap()
            .contains("connection lost")
    );
    waiting.control.resume();
    waiting.finished(false);
}

#[test]
fn a_slow_space_recheck_cannot_repause_after_resume_or_cancellation() {
    for cancel in [false, true] {
        let waiting = Waiting::new();
        waiting.control.recheck_space();
        waiting
            .queries
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        waiting.control.recheck_space(); // Coalesce clicks while a query is in flight.
        if cancel {
            waiting.control.cancel();
        } else {
            waiting.control.resume();
        }
        waiting.replies.send(Ok(Some(0))).unwrap();
        waiting.finished(cancel);
        assert!(waiting.queries.try_recv().is_err());
    }
}

#[test]
fn low_space_cancel_wakes_the_worker_without_a_recheck() {
    let waiting = Waiting::new();
    waiting.control.cancel();
    waiting.finished(true);
}
