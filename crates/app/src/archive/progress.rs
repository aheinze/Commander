use super::*;
use dualpane_engine::JobControl;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct ArchiveProgress {
    pub bytes_done: u64,
    pub items_done: u64,
    pub current_path: Option<VPath>,
    pub finishing: bool,
}

type PasswordPrompt<'a> = Box<dyn FnMut(&VPath, bool) -> Option<Password> + 'a>;

pub struct ArchiveTask<'a> {
    pub(super) cancel: CancelToken,
    pub(super) password: Option<Password>,
    pub(super) password_prompt: Option<PasswordPrompt<'a>>,
    control: Option<JobControl>,
    journal: Option<std::sync::Arc<dualpane_engine::journal::JobJournal>>,
    progress: ArchiveProgress,
    callback: Box<dyn FnMut(ArchiveProgress) + 'a>,
    last_emit: Option<Instant>,
}

impl<'a> ArchiveTask<'a> {
    pub fn new(cancel: &CancelToken) -> Self {
        Self {
            cancel: cancel.clone(),
            password: None,
            password_prompt: None,
            control: None,
            journal: None,
            progress: ArchiveProgress::default(),
            callback: Box::new(|_| {}),
            last_emit: None,
        }
    }

    pub fn with_progress(control: &JobControl, callback: impl FnMut(ArchiveProgress) + 'a) -> Self {
        Self {
            control: Some(control.clone()),
            callback: Box::new(callback),
            ..Self::new(&control.cancel_token())
        }
    }

    pub fn set_password(&mut self, password: Option<Password>) {
        self.password = password;
    }

    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    pub fn password(&self) -> Option<Password> {
        self.password.clone()
    }

    pub fn set_password_prompt(
        &mut self,
        prompt: impl FnMut(&VPath, bool) -> Option<Password> + 'a,
    ) {
        self.password_prompt = Some(Box::new(prompt));
    }

    pub fn set_journal(&mut self, journal: std::sync::Arc<dualpane_engine::journal::JobJournal>) {
        self.journal = Some(journal);
    }
    pub(super) fn journal(&self) -> Option<std::sync::Arc<dualpane_engine::journal::JobJournal>> {
        self.journal.clone()
    }

    pub(super) fn control(&self) -> Option<JobControl> {
        self.control.clone()
    }

    pub(super) fn check(&self) -> Result<(), dualpane_core::Cancelled> {
        self.control
            .as_ref()
            .map_or_else(|| self.cancel.check(), JobControl::checkpoint)
    }

    pub(super) fn begin(&mut self, path: &VPath) {
        self.progress.current_path = Some(path.clone());
        self.emit();
    }

    pub(super) fn advanced(&mut self, bytes: u64) {
        self.progress.bytes_done = self.progress.bytes_done.saturating_add(bytes);
        self.emit();
    }

    pub(super) fn item_done(&mut self) {
        self.progress.items_done = self.progress.items_done.saturating_add(1);
        self.emit();
    }

    pub(super) fn finishing(&mut self) -> Result<(), String> {
        self.check()
            .map_err(|_| "Archive operation cancelled".to_owned())?;
        self.progress.finishing = true;
        self.flush();
        Ok(())
    }

    fn emit(&mut self) {
        if self
            .last_emit
            .is_none_or(|last| last.elapsed() >= Duration::from_millis(50))
        {
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        (self.callback)(self.progress.clone());
        self.last_emit = Some(Instant::now());
    }
}
