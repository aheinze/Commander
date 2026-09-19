//! Graceful window close with a bounded wait for unresponsive filesystems.
use super::*;
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(5);

pub(super) struct State {
    window: glib::WeakRef<adw::ApplicationWindow>,
    allow_close: Rc<Cell<bool>>,
    pub confirming: bool,
    pub started: Option<Instant>,
    pub saving: bool,
    timer: Option<glib::SourceId>,
}
impl State {
    pub fn new(window: &adw::ApplicationWindow, input: &relm4::Sender<AppMsg>) -> Self {
        let allow_close = Rc::new(Cell::new(false));
        let allowed = allow_close.clone();
        let input = input.clone();
        window.connect_close_request(move |_| {
            if allowed.get() {
                glib::Propagation::Proceed
            } else {
                let _ = input.send(AppMsg::RequestClose);
                glib::Propagation::Stop
            }
        });
        Self {
            window: window.downgrade(),
            allow_close,
            confirming: false,
            started: None,
            saving: false,
            timer: None,
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.remove();
        }
    }
}
impl AppModel {
    pub(super) fn request_close(&mut self, sender: &ComponentSender<Self>) {
        if self.shutdown.started.is_some() || self.shutdown.confirming {
            return;
        }
        let busy = self.active_operations > 0
            || self.history_busy
            || self.tool_cancel.is_some()
            || self.remote_requests.iter().any(Option::is_some);
        if !busy {
            self.begin_shutdown(sender);
            return;
        }
        self.shutdown.confirming = true;
        let dialog = AlertSheet::new(
            Some("Close Commander while work is running?"),
            Some(
                "Closing will cancel active operations and connections. Completed transfers are kept; unfinished work remains available in Recovery.",
            ),
        );
        dialog.add_response("keep", "Keep working");
        dialog.add_response("quit", "Cancel operations and quit");
        dialog.set_default_response(Some("keep"));
        dialog.set_close_response("keep");
        dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
        let input = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            let _ = input.send(AppMsg::CloseDecision(response == "quit"));
        });
        dialog.present(self.shutdown.window.upgrade().as_ref());
    }

    pub(super) fn cancel_background_work(&mut self) {
        self.live_updates.stop();
        for operation in self.operations.values() {
            if operation.is_active() {
                operation.control.cancel();
            }
        }
        if let Some(cancel) = self.tool_cancel.take() {
            cancel.cancel();
        }
        if let Some(cancel) = self.search_cancel.take() {
            cancel.cancel();
        }
        self.remote_requests = [None, None];
        self.terminal_tabs.clear();
        self.archive_mounts.cancel_reloads();
        for pane in &mut self.panes {
            pane.cancel_work();
            pane.git.cancel();
        }
        if let Some(scheduler) = &mut self.thumbnail_scheduler {
            scheduler.stop();
        }
    }

    pub(super) fn begin_shutdown(&mut self, sender: &ComponentSender<Self>) {
        if self.shutdown.started.is_some() {
            return;
        }
        self.shutdown.started = Some(Instant::now());
        self.cancel_background_work();
        if let Some(window) = self.shutdown.window.upgrade() {
            // A stale conflict or credential sheet must not consume the final close request.
            let dialogs = window.dialogs();
            let dialogs = (0..dialogs.n_items())
                .filter_map(|index| dialogs.item(index).and_downcast::<adw::Dialog>())
                .collect::<Vec<_>>();
            for dialog in dialogs {
                dialog.force_close();
            }
            window.set_title(Some("Commander — Stopping operations…"));
            window.set_sensitive(false);
        }
        self.shutdown_tick(sender);
    }

    pub(super) fn shutdown_tick(&mut self, sender: &ComponentSender<Self>) {
        let Some(started) = self.shutdown.started else {
            return;
        };
        let elapsed = started.elapsed();
        self.reap_aux_workers();
        let idle = self.active_operations == 0
            && !self.history_busy
            && self.aux_workers.is_empty()
            && self.panes.iter().all(|pane| pane.workers.is_empty())
            && self
                .thumbnail_scheduler
                .as_ref()
                .is_none_or(ThumbnailScheduler::is_finished);
        // Reserve one second of the deadline for the final settings write, even if I/O stalls.
        if !self.shutdown.saving && (idle || elapsed >= SHUTDOWN_LIMIT - Duration::from_secs(1)) {
            self.persist_session();
            self.session_worker.request_shutdown();
            self.shutdown.saving = true;
        }
        if (idle && self.shutdown.saving && self.session_worker.is_finished())
            || elapsed >= SHUTDOWN_LIMIT
        {
            if let Some(timer) = self.shutdown.timer.take() {
                timer.remove();
            }
            self.shutdown.allow_close.set(true);
            if let Some(window) = self.shutdown.window.upgrade() {
                window.close();
            }
            return;
        }
        if self.shutdown.timer.is_none() {
            let input = sender.input_sender().clone();
            self.shutdown.timer = Some(glib::timeout_add_local(
                Duration::from_millis(50),
                move || {
                    let _ = input.send(AppMsg::ShutdownTick);
                    glib::ControlFlow::Continue
                },
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ux_tests::{button, launch, wait};
    use relm4::ComponentController;

    #[test]
    #[ignore = "requires an isolated GTK session; run in the native suite"]
    fn gtk_close_keeps_work_running_by_default_and_bounds_stalled_shutdown() {
        let fixture = tempfile::tempdir().unwrap();
        let app = launch(fixture.path());
        let control = JobControl::new();
        control.pause();
        let id = JobId::next();
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        {
            let mut parts = app.state().get_mut();
            parts.model.active_operations = 1;
            parts.model.operations.insert(
                id,
                OperationStatus {
                    recovery: RetryState::default(),
                    phase: JobPhase::Copying,
                    kind: OperationKind::Files(JobKind::Copy),
                    state: JobState::Paused,
                    progress: JobProgress::default(),
                    control: control.clone(),
                    commands: std::sync::mpsc::channel().0,
                    retry: OperationRetry::Copy {
                        pane: PaneId::Left,
                        sources: vec![],
                        destination: fixture.path().into(),
                    },
                    waiting_for_conflict: false,
                    error: None,
                },
            );
            parts.model.aux_workers.push(thread::spawn(move || {
                let _ = blocked.recv();
            }));
        }
        app.widget().close();
        wait(|| app.widget().visible_dialog().is_some());
        assert!(!control.cancel_token().is_cancelled());
        let dialog = app.widget().visible_dialog().unwrap();
        assert_eq!(
            dialog.default_widget(),
            Some(button(&dialog, "Keep working").unwrap().upcast())
        );
        button(&dialog, "Keep working").unwrap().emit_clicked();
        wait(|| !app.model().shutdown.confirming);
        assert!(app.widget().is_visible());
        assert!(!control.cancel_token().is_cancelled());
        let pending = adw::Dialog::builder()
            .can_close(false)
            .child(&gtk::Label::new(Some("A previous operation prompt")))
            .build();
        pending.present(Some(app.widget()));
        app.emit(AppMsg::RequestClose);
        wait(|| {
            app.widget()
                .visible_dialog()
                .as_ref()
                .is_some_and(|dialog| dialog != &pending)
        });
        button(
            &app.widget().visible_dialog().unwrap(),
            "Cancel operations and quit",
        )
        .unwrap()
        .emit_clicked();
        wait(|| app.model().shutdown.started.is_some());
        assert!(app.widgets().shutdown_status.is_visible());
        assert!(control.cancel_token().is_cancelled());
        assert!(!control.is_paused());
        let path: VPath = fixture.path().into();
        let metadata = LocalFs.stat(&path, false).unwrap();
        app.emit(AppMsg::OperationEvent {
            kind: JobKind::Copy,
            event: JobEvent::Conflict {
                id,
                conflict_id: ConflictId::next(),
                conflict: Box::new(Conflict {
                    source: path.clone(),
                    destination: path,
                    source_metadata: metadata.clone(),
                    destination_metadata: metadata,
                }),
            },
        });
        app.emit(AppMsg::OperationEvent {
            kind: JobKind::Copy,
            event: JobEvent::Phase {
                id,
                phase: JobPhase::Finishing,
                path: None,
            },
        });
        wait(|| app.model().operations[&id].phase == JobPhase::Finishing);
        assert!(!app.model().operations[&id].waiting_for_conflict);
        wait(|| app.widget().visible_dialog().is_none());
        let started = Instant::now();
        wait(|| !app.widget().is_visible());
        assert!(started.elapsed() < Duration::from_secs(6));
        assert!(app.model().session_worker.is_finished());
        let drop_started = Instant::now();
        drop(app);
        assert!(drop_started.elapsed() < Duration::from_millis(500));
        release.send(()).unwrap();
    }
}
