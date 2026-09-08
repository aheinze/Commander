//! Update controls live with the About dialog; closing it cancels its worker.

use super::*;
use crate::updates::{self as updater, Available, Cache, Outcome};
use std::path::PathBuf;

#[cfg(test)]
mod tests;

enum Event {
    Loaded(Cache),
    Checked(Cache, Box<updater::Result<Outcome>>),
    Progress(u64),
    Downloaded(updater::Result<PathBuf>),
    SaveFailed,
}

pub(super) struct Panel {
    pub root: gtk::Box,
    status: notifications::Feedback,
    checked: gtk::Label,
    check: gtk::Button,
    details: gtk::Box,
    description: gtk::Label,
    formats: gtk::DropDown,
    notes: gtk::Label,
    release: gtk::LinkButton,
    download: gtk::Button,
    skip: gtk::Button,
    stop: gtk::Button,
    reveal: gtk::Button,
    progress: gtk::ProgressBar,
    cache: RefCell<Cache>,
    available: RefCell<Option<Available>>,
    destination: RefCell<Option<PathBuf>>,
    cancel: RefCell<CancelToken>,
    busy: Cell<bool>,
    closed: Cell<bool>,
    send: relm4::Sender<Event>,
}

impl Panel {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.add_css_class("settings-updates");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 4);
        labels.set_hexpand(true);
        let status = notifications::Feedback::default();
        let title = label("Updates");
        title.add_css_class("settings-row-title");
        let checked = label("Checks run only when you request them.");
        checked.add_css_class("settings-description");
        labels.append(&title);
        labels.append(&checked);
        let check = gtk::Button::with_label("Check for updates");
        check.set_valign(gtk::Align::Center);
        check.set_sensitive(false);
        top.append(&labels);
        top.append(&check);
        root.append(&top);
        let details = gtk::Box::new(gtk::Orientation::Vertical, 10);
        details.set_visible(false);
        let description = label("");
        description.add_css_class("settings-description");
        details.append(&description);
        let formats = gtk::DropDown::from_strings(&[]);
        formats.update_property(&[gtk::accessible::Property::Label("Update package format")]);
        details.append(&formats);
        let notes = label("");
        notes.set_selectable(true);
        let expander = gtk::Expander::new(Some("Release notes"));
        let notes_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(140)
            .max_content_height(220)
            .propagate_natural_height(true)
            .child(&notes)
            .build();
        expander.set_child(Some(&notes_scroll));
        details.append(&expander);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let release = gtk::LinkButton::with_label(updater::RELEASES_URL, "View release");
        let download = gtk::Button::with_label("Download…");
        let skip = gtk::Button::with_label("Skip this version");
        actions.append(&release);
        actions.append(&download);
        actions.append(&skip);
        details.append(&actions);
        root.append(&details);
        let progress = gtk::ProgressBar::new();
        progress.set_show_text(true);
        progress.set_visible(false);
        progress.update_property(&[gtk::accessible::Property::Label("Update download progress")]);
        root.append(&progress);
        let stop = gtk::Button::with_label("Cancel download");
        stop.set_halign(gtk::Align::Start);
        stop.set_visible(false);
        root.append(&stop);
        let reveal = gtk::Button::with_label("Show download in folder");
        reveal.set_halign(gtk::Align::Start);
        reveal.set_visible(false);
        root.append(&reveal);
        let (send, receive) = relm4::channel();
        let panel = Rc::new(Self {
            root,
            status,
            checked,
            check,
            details,
            description,
            formats,
            notes,
            release,
            download,
            skip,
            stop,
            reveal,
            progress,
            cache: RefCell::default(),
            available: RefCell::default(),
            destination: RefCell::default(),
            cancel: RefCell::new(CancelToken::new()),
            busy: Cell::new(false),
            closed: Cell::new(false),
            send,
        });
        panel.check.connect_clicked({
            let weak = Rc::downgrade(&panel);
            move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.start_check();
                }
            }
        });
        panel.download.connect_clicked({
            let weak = Rc::downgrade(&panel);
            move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.choose_download();
                }
            }
        });
        panel.stop.connect_clicked({
            let weak = Rc::downgrade(&panel);
            move |button| {
                if let Some(panel) = weak.upgrade() {
                    panel.cancel.borrow().cancel();
                    button.set_sensitive(false);
                    panel.status.info("Cancelling download…");
                }
            }
        });
        panel.skip.connect_clicked({
            let weak = Rc::downgrade(&panel);
            move |_| {
                if let Some(panel) = weak.upgrade() {
                    let version = panel
                        .available
                        .borrow()
                        .as_ref()
                        .map(|update| update.version.to_string());
                    panel.cache.borrow_mut().skipped = version;
                    panel.show_cached_status();
                    panel.status.info("This version will be skipped");
                    panel.details.set_visible(false);
                    let cache = panel.cache.borrow().clone();
                    let send = panel.send.clone();
                    panel.spawn(move || save_cache(&cache, &send));
                }
            }
        });
        panel.reveal.connect_clicked({
            let weak = Rc::downgrade(&panel);
            move |_| {
                if let Some(panel) = weak.upgrade()
                    && let Some(path) = panel.destination.borrow().as_ref()
                {
                    reveal_in_file_manager(VPath::from(path.as_path()));
                }
            }
        });
        glib::spawn_future_local({
            let weak = Rc::downgrade(&panel);
            async move {
                while let Some(event) = receive.recv().await {
                    let Some(panel) = weak.upgrade() else {
                        break;
                    };
                    if panel.closed.get() {
                        break;
                    }
                    panel.handle(event);
                }
            }
        });
        let send = panel.send.clone();
        panel.spawn(move || {
            let cache = Cache::path()
                .map(|path| Cache::load(&path))
                .unwrap_or_default();
            let _ = send.send(Event::Loaded(cache));
        });
        panel
    }

    pub fn close(&self) {
        self.closed.set(true);
        self.cancel.borrow().cancel();
    }

    fn spawn(&self, work: impl FnOnce() + Send + 'static) {
        if let Err(error) = thread::Builder::new()
            .name("commander-updates".into())
            .spawn(work)
        {
            self.set_busy(false);
            self.status
                .error(&format!("Could not start the update worker: {error}"));
        }
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        self.check.set_sensitive(!busy);
        self.formats.set_sensitive(!busy);
        self.download.set_sensitive(!busy);
        self.skip.set_sensitive(!busy);
    }

    fn show_cached_status(&self) {
        let cache = self.cache.borrow();
        let checked = cache
            .checked_at
            .and_then(|time| glib::DateTime::from_unix_local(time as i64).ok())
            .and_then(|date| date.format("%x %X").ok());
        self.checked.set_text(&checked.map_or_else(
            || "Checks run only when you request them.".into(),
            |date| format!("Last checked {date} · Stable releases"),
        ));
    }

    fn start_check(&self) {
        if self.busy.get() {
            return;
        }
        self.set_busy(true);
        self.details.set_visible(false);
        self.reveal.set_visible(false);
        self.progress.set_visible(false);
        self.status.info("Checking GitHub Releases…");
        let cancel = CancelToken::new();
        *self.cancel.borrow_mut() = cancel.clone();
        let mut cache = self.cache.borrow().clone();
        let send = self.send.clone();
        self.spawn(move || {
            let result = updater::check(&mut cache, &cancel);
            if cancel.is_cancelled() {
                return;
            }
            let saved = Cache::path().is_none_or(|path| cache.save(&path).is_ok());
            let _ = send.send(Event::Checked(cache, Box::new(result)));
            if !saved {
                let _ = send.send(Event::SaveFailed);
            }
        });
    }

    fn show_available(&self, update: Available) {
        self.status
            .info(&format!("Commander {} is available", update.version));
        let text = if update.notice.is_empty() {
            update.installation.description.clone()
        } else {
            format!("{}\n{}", update.notice, update.installation.description)
        };
        self.description.set_text(&text);
        self.notes.set_text(if update.notes.is_empty() {
            "No release notes were provided. See the release page for details."
        } else {
            &update.notes
        });
        self.release.set_uri(&update.url());
        let labels: Vec<_> = update
            .assets
            .iter()
            .map(|asset| {
                format!(
                    "{} · {}",
                    asset.format.label(),
                    format_size(asset.size, EntryKind::File)
                )
            })
            .collect();
        self.formats.set_model(Some(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        )));
        let preferred = update
            .assets
            .iter()
            .position(|asset| Some(asset.format) == update.installation.preferred)
            .unwrap_or(0);
        self.formats.set_selected(preferred as u32);
        self.formats.set_visible(!update.assets.is_empty());
        self.download.set_visible(!update.assets.is_empty());
        self.details.set_visible(true);
        *self.available.borrow_mut() = Some(update);
    }

    fn choose_download(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let Some(update) = self.available.borrow().clone() else {
            return;
        };
        let Some(asset) = update.assets.get(self.formats.selected() as usize).cloned() else {
            return;
        };
        let Some(parent) = self.root.root().and_downcast::<gtk::Window>() else {
            return;
        };
        let chooser = gtk::FileDialog::builder()
            .title("Save Commander update")
            .initial_name(&asset.name)
            .accept_label("Download")
            .build();
        self.set_busy(true);
        glib::spawn_future_local({
            let weak = Rc::downgrade(self);
            async move {
                let result = chooser.save_future(Some(&parent)).await;
                let Some(panel) = weak.upgrade() else {
                    return;
                };
                if panel.closed.get() {
                    return;
                }
                let file = match result {
                    Ok(file) => file,
                    Err(error) => {
                        panel.set_busy(false);
                        if !error.matches(gtk::DialogError::Dismissed)
                            && !error.matches(gtk::DialogError::Cancelled)
                        {
                            panel
                                .status
                                .error(&format!("Could not choose a download location: {error}"));
                        }
                        return;
                    }
                };
                let Some(path) = file.path() else {
                    panel.set_busy(false);
                    panel
                        .status
                        .error("Choose a local folder for the download.");
                    return;
                };
                panel.status.info("Downloading update…");
                panel.progress.set_fraction(0.0);
                panel.progress.set_text(Some("Starting download…"));
                panel.progress.set_visible(true);
                panel.stop.set_sensitive(true);
                panel.stop.set_visible(true);
                let cancel = CancelToken::new();
                *panel.cancel.borrow_mut() = cancel.clone();
                let send = panel.send.clone();
                panel.spawn(move || {
                    let mut last = Instant::now();
                    let result = updater::download(&update, &asset, &path, &cancel, |received| {
                        if last.elapsed() >= Duration::from_millis(100) || received == asset.size {
                            let _ = send.send(Event::Progress(received));
                            last = Instant::now();
                        }
                    });
                    let _ = send.send(Event::Downloaded(result));
                });
            }
        });
    }

    fn handle(&self, event: Event) {
        if self.closed.get() {
            return;
        }
        match event {
            Event::Loaded(cache) => {
                *self.cache.borrow_mut() = cache;
                self.set_busy(false);
                self.show_cached_status();
            }
            Event::Checked(cache, result) => {
                *self.cache.borrow_mut() = cache;
                self.set_busy(false);
                self.show_cached_status();
                match *result {
                    Ok(Outcome::Current) => self.status.info("Commander is up to date"),
                    Ok(Outcome::NoRelease) => self.status.info("No stable release is published yet"),
                    Ok(Outcome::Available(update)) => self.show_available(update),
                    Err(error) => {
                        let retry = error.retry_at.and_then(|time| glib::DateTime::from_unix_local(time as i64).ok())
                            .and_then(|date| date.format("%H:%M:%S").ok());
                        self.status.error(&retry.map_or(error.message.clone(), |time| format!("{} Retry after {time}.", error.message)));
                    }
                }
            }
            Event::Progress(received) => {
                if let Some(update) = self.available.borrow().as_ref()
                    && let Some(asset) = update.assets.get(self.formats.selected() as usize) {
                    self.progress.set_fraction((received as f64 / asset.size as f64).clamp(0.0, 1.0));
                    self.progress.set_text(Some(&format!("{} of {}", format_size(received, EntryKind::File), format_size(asset.size, EntryKind::File))));
                }
            }
            Event::Downloaded(result) => {
                self.set_busy(false);
                self.stop.set_visible(false);
                self.progress.set_visible(false);
                match result {
                    Ok(path) => {
                        self.status.success("Download verified and saved. Install the downloaded package manually.");
                        *self.destination.borrow_mut() = Some(path);
                        self.reveal.set_visible(true);
                    }
                    Err(error) => self.status.error(&error.message),
                }
            }
            Event::SaveFailed => self.status.error("Could not save update preferences. This check still applies to the current session."),
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.cancel.get_mut().cancel();
    }
}

fn label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label
}

fn save_cache(cache: &Cache, send: &relm4::Sender<Event>) {
    if let Some(path) = Cache::path()
        && cache.save(&path).is_err()
    {
        let _ = send.send(Event::SaveFailed);
    }
}
