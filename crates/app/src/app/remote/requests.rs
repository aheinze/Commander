//! Owned mount attempts: cancellation, deadline, stale results and their destination tab.
use super::*;

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

pub(in crate::app) struct Request {
    pub id: u64,
    pub uri: String,
    pub tab: u64,
    task: glib::JoinHandle<()>,
    timer: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Drop for Request {
    fn drop(&mut self) {
        // Dropping GIO's future cancels its in-flight operation.
        self.task.abort();
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
    }
}

impl AppModel {
    pub(in crate::app) fn begin_remote_request(
        &mut self,
        pane: PaneId,
        connection: RemoteConnection,
        input: &relm4::Sender<AppMsg>,
    ) {
        let uri = connection.uri.clone();
        self.begin_mount(
            pane,
            uri,
            mount_connection(connection),
            CONNECTION_TIMEOUT,
            input,
        );
    }

    pub(in crate::app) fn begin_mount(
        &mut self,
        pane: PaneId,
        uri: String,
        mount: impl std::future::Future<Output = Result<VPath, String>> + 'static,
        timeout: Duration,
        input: &relm4::Sender<AppMsg>,
    ) {
        self.remote_requests[pane.index()].take();
        self.remote_serial = self.remote_serial.wrapping_add(1);
        let id = self.remote_serial;
        let tab = self.pane(pane).active().id;
        let complete = input.clone();
        let task = glib::spawn_future_local(async move {
            let result = mount.await;
            let _ = complete.send(AppMsg::RemoteConnected { pane, id, result });
        });
        let timer = Rc::new(RefCell::new(None));
        let elapsed = timer.clone();
        let input = input.clone();
        *timer.borrow_mut() = Some(glib::timeout_add_local_once(timeout, move || {
            elapsed.borrow_mut().take();
            let _ = input.send(AppMsg::CancelRemote {
                pane,
                id,
                timed_out: true,
            });
        }));
        self.remote_requests[pane.index()] = Some(Request {
            id,
            uri,
            tab,
            task,
            timer,
        });
    }

    pub(in crate::app) fn connect_remote_in_pane(
        &mut self,
        pane: PaneId,
        tab: u64,
        uri: String,
        sender: &ComponentSender<Self>,
    ) {
        if !self
            .pane(pane)
            .tabs
            .iter()
            .any(|candidate| candidate.id == tab)
        {
            return;
        }
        match RemoteConnection::parse(&uri) {
            Ok(connection) => {
                self.begin_remote_request(pane, connection, sender.input_sender());
                self.remote_requests[pane.index()].as_mut().unwrap().tab = tab;
            }
            Err(error) => self.pane_mut(pane).error = Some(error.into()),
        }
    }

    pub(in crate::app) fn cancel_remote(
        &mut self,
        pane: PaneId,
        id: u64,
        timed_out: bool,
        sender: &ComponentSender<Self>,
    ) {
        if !self.remote_requests[pane.index()]
            .as_ref()
            .is_some_and(|request| request.id == id)
        {
            return;
        }
        let request = self.remote_requests[pane.index()].take().unwrap();
        let uri = request.uri.clone();
        let tab = request.tab;
        drop(request);
        if timed_out {
            show_remote_error(pane, tab, uri, "The server did not finish connecting within 60 seconds. Check the connection and try again.".into(), sender);
        } else {
            notifications::show(notifications::Kind::Info, "Connection cancelled");
        }
    }

    pub(in crate::app) fn on_remote_connected(
        &mut self,
        pane: PaneId,
        id: u64,
        result: Result<VPath, String>,
        sender: &ComponentSender<Self>,
    ) {
        if !self.remote_requests[pane.index()]
            .as_ref()
            .is_some_and(|request| request.id == id)
        {
            return;
        }
        let request = self.remote_requests[pane.index()].take().unwrap();
        match result {
            Ok(path) => {
                let Some(tab) = self
                    .pane(pane)
                    .tabs
                    .iter()
                    .position(|tab| tab.id == request.tab)
                else {
                    return;
                };
                if tab == self.pane(pane).active_tab {
                    let active = self.active_pane;
                    let focus = self.pane(pane).focus_files_epoch;
                    self.navigate(pane, path, sender);
                    self.active_pane = active;
                    if active != pane {
                        self.pane_mut(pane).focus_files_epoch = focus;
                    }
                } else {
                    self.pane_mut(pane).tabs[tab].navigate(path.clone());
                    self.pane_mut(pane).tabs_revision =
                        self.pane(pane).tabs_revision.wrapping_add(1);
                    self.record_recent(path);
                    self.persist_session();
                }
            }
            Err(error) => {
                self.push_operation_log(format!("Could not connect to {}: {error}", request.uri));
                show_remote_error(pane, request.tab, request.uri.clone(), error, sender);
            }
        }
    }
}

pub(in crate::app) struct StatusWidgets {
    pane: PaneId,
    pub root: gtk::Box,
    label: gtk::Label,
    id: Rc<Cell<u64>>,
}

impl StatusWidgets {
    pub fn new(pane: PaneId, input: &relm4::Sender<AppMsg>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        root.set_visible(false);
        root.set_margin_start(12);
        root.set_margin_end(12);
        root.set_margin_top(4);
        root.set_margin_bottom(4);
        let spinner = gtk::Spinner::new();
        spinner.start();
        root.append(&spinner);
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_width_chars(1);
        root.append(&label);
        let cancel = gtk::Button::with_label("Cancel");
        let id = Rc::new(Cell::new(0));
        let request_id = id.clone();
        let input = input.clone();
        cancel.connect_clicked(move |_| {
            let _ = input.send(AppMsg::CancelRemote {
                pane,
                id: request_id.get(),
                timed_out: false,
            });
        });
        root.append(&cancel);
        Self {
            pane,
            root,
            label,
            id,
        }
    }
    pub fn render(&self, request: Option<&Request>, names: &BTreeMap<String, String>) {
        self.root.set_visible(request.is_some());
        if let Some(request) = request {
            self.id.set(request.id);
            let name = names
                .get(&request.uri)
                .map_or(request.uri.as_str(), String::as_str);
            self.label
                .set_text(&format!("{}: Connecting to {name}…", self.pane.label()));
            self.label.set_tooltip_text(Some(&request.uri));
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
    fn gtk_remote_attempts_cancel_timeout_and_keep_the_originating_tab() {
        let fixture = tempfile::tempdir().unwrap();
        let destination = fixture.path().join("remote");
        std::fs::create_dir(&destination).unwrap();
        let app = launch(fixture.path());
        let start = |timeout| {
            app.state().get_mut().model.begin_mount(
                PaneId::Left,
                "sftp://test.example/".into(),
                std::future::pending(),
                timeout,
                app.sender(),
            );
            app.model().remote_requests[0].as_ref().unwrap().id
        };
        let first = start(CONNECTION_TIMEOUT);
        // Superseding attempts ignore an old success instead of changing the pane.
        let second = start(CONNECTION_TIMEOUT);
        app.emit(AppMsg::RemoteConnected {
            pane: PaneId::Left,
            id: first,
            result: Ok(destination.clone().into()),
        });
        app.emit(AppMsg::NewTab(PaneId::Left));
        wait(|| app.model().panes[0].tabs.len() == 2);
        assert_eq!(app.model().remote_requests[0].as_ref().unwrap().id, second);
        assert_eq!(app.model().panes[0].tabs[0].path, fixture.path().into());
        app.state().get_mut().model.active_pane = PaneId::Right;
        app.emit(AppMsg::RemoteConnected {
            pane: PaneId::Left,
            id: second,
            result: Ok(destination.clone().into()),
        });
        wait(|| app.model().remote_requests[0].is_none());
        assert_eq!(
            app.model().panes[0].tabs[0].path,
            destination.clone().into()
        );
        assert_eq!(app.model().panes[0].active_tab, 1);
        assert_eq!(app.model().panes[0].active().path, fixture.path().into());
        assert_eq!(app.model().active_pane, PaneId::Right);

        struct Dropped(Rc<Cell<bool>>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let guard = Dropped(dropped.clone());
        app.state().get_mut().model.begin_mount(
            PaneId::Left,
            "sftp://test.example/".into(),
            async move {
                let _guard = guard;
                std::future::pending().await
            },
            CONNECTION_TIMEOUT,
            app.sender(),
        );
        let cancel = app.model().remote_requests[0].as_ref().unwrap().id;
        // Trigger a normal update to render the owned request's status bar.
        app.emit(AppMsg::RemoteConnected {
            pane: PaneId::Left,
            id: first,
            result: Ok(destination.clone().into()),
        });
        wait(|| app.widgets().remote_connecting[0].root.is_visible());
        button(&app.widgets().remote_connecting[0].root, "Cancel")
            .unwrap()
            .emit_clicked();
        wait(|| app.model().remote_requests[0].is_none());
        wait(|| dropped.get());
        app.emit(AppMsg::RemoteConnected {
            pane: PaneId::Left,
            id: cancel,
            result: Ok(destination.clone().into()),
        });
        app.emit(AppMsg::SelectTab(PaneId::Left, 0));
        wait(|| app.model().panes[0].active_tab == 0);
        assert_eq!(app.model().panes[0].tabs[1].path, fixture.path().into());

        let closing = start(CONNECTION_TIMEOUT);
        app.emit(AppMsg::CloseTabAt(PaneId::Left, 0));
        wait(|| app.model().remote_requests[0].is_none());
        assert_eq!(app.model().panes[0].tabs.len(), 1);
        app.emit(AppMsg::RemoteConnected {
            pane: PaneId::Left,
            id: closing,
            result: Ok(destination.clone().into()),
        });
        app.emit(AppMsg::SelectTab(PaneId::Left, 0));

        start(Duration::from_millis(20));
        wait(|| app.model().remote_requests[0].is_none());
        wait(|| {
            notifications::test_messages()
                .iter()
                .any(|message| message.contains("did not finish connecting"))
        });
        assert!(app.widget().visible_dialog().is_none());
        app.widget().close();
        wait(|| !app.widget().is_visible());
    }
}
