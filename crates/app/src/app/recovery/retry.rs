//! Review saved transfer roots before claiming their journal and starting a new job.
use super::*;
use dualpane_engine::recovery::{RecoveryClaim, RecoveryPlan};
use std::path::PathBuf;

#[derive(Default)]
pub(in crate::app) struct State {
    serial: u64,
    pending: Option<(u64, PaneId, PathBuf)>,
    prepared: Option<Arc<RecoveryPlan>>,
}

impl AppModel {
    pub(in crate::app) fn prepare_recovery_retry(
        &mut self,
        path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        if self.recovery_retry.pending.is_some() {
            return;
        }
        if !self
            .recovery_records
            .iter()
            .any(|record| record.path == path)
        {
            return;
        }
        if self.history_busy {
            notifications::error("Wait for undo or redo to finish before retrying files");
            return;
        }
        self.recovery_retry.serial += 1;
        let id = self.recovery_retry.serial;
        self.recovery_retry.pending = Some((id, self.active_pane, path.clone()));
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("commander-review-retry".into())
            .spawn(move || {
                let result = RecoveryPlan::load(&path)
                    .map(Arc::new)
                    .map_err(|e| e.to_string());
                let _ = input.send(AppMsg::RecoveryRetryPrepared { id, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.recovery_retry.pending = None;
                notifications::error(&format!("Could not prepare retry: {error}"));
            }
        }
    }

    pub(in crate::app) fn on_recovery_retry_prepared(
        &mut self,
        id: u64,
        result: Result<Arc<RecoveryPlan>, String>,
        sender: &ComponentSender<Self>,
    ) {
        if !self
            .recovery_retry
            .pending
            .as_ref()
            .is_some_and(|pending| pending.0 == id)
        {
            return;
        }
        let plan = match result {
            Ok(plan) => plan,
            Err(error) => {
                self.recovery_retry.pending = None;
                notifications::error(&format!("Could not prepare retry: {error}"));
                return;
            }
        };
        let record = plan.record();
        let mut body = format!(
            "{} {} source(s) to:\n{}\n\nCommander will rescan these sources and verify {} saved completed item(s). Unchanged completed files are reused. Other existing destinations use the usual conflict choices.\n\n",
            job_kind_label(record.kind),
            record.sources.len(),
            record.destination.as_ref().unwrap(),
            record.transfers.len()
        );
        for source in record.sources.iter().take(12) {
            body.push_str(&format!("Source: {source}\n"));
        }
        if record.sources.len() > 12 {
            body.push_str(&format!(
                "…and {} more sources.\n",
                record.sources.len() - 12
            ));
        }
        body.push_str(&format!("\nFull operation record: {}\n\nConnect any required remote locations before continuing. Retained originals remain in Recovery.", record.path.display()));
        let dialog = AlertSheet::new(Some("Retry remaining items?"), Some(&body));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("retry", "Retry remaining items");
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("retry", adw::ResponseAppearance::Suggested);
        let input = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            let _ = input.send(AppMsg::RecoveryRetryDecision {
                id,
                confirmed: response == "retry",
            });
        });
        self.recovery_retry.prepared = Some(plan);
        dialog.present(relm4::main_application().active_window().as_ref());
    }

    pub(in crate::app) fn recovery_retry_decision(
        &mut self,
        id: u64,
        confirmed: bool,
        sender: &ComponentSender<Self>,
    ) {
        if !self
            .recovery_retry
            .pending
            .as_ref()
            .is_some_and(|pending| pending.0 == id)
        {
            return;
        }
        let Some(plan) = self.recovery_retry.prepared.take() else {
            return;
        };
        if !confirmed {
            self.recovery_retry.pending = None;
            return;
        }
        let input = sender.input_sender().clone();
        match thread::Builder::new()
            .name("commander-claim-retry".into())
            .spawn(move || {
                let result = plan.claim().map(Box::new).map_err(|e| e.to_string());
                let _ = input.send(AppMsg::RecoveryRetryClaimed { id, result });
            }) {
            Ok(worker) => self.aux_workers.push(worker),
            Err(error) => {
                self.recovery_retry.pending = None;
                notifications::error(&format!("Could not start retry: {error}"));
            }
        }
    }

    pub(in crate::app) fn on_recovery_retry_claimed(
        &mut self,
        id: u64,
        result: Result<Box<RecoveryClaim>, String>,
        sender: &ComponentSender<Self>,
    ) {
        if !self
            .recovery_retry
            .pending
            .as_ref()
            .is_some_and(|pending| pending.0 == id)
        {
            return;
        }
        let (_, pane, _) = self.recovery_retry.pending.take().unwrap();
        let claim = match result {
            Ok(claim) => claim,
            Err(error) => {
                notifications::error(&format!("Could not start retry: {error}"));
                return;
            }
        };
        if self.history_busy {
            notifications::error("Wait for undo or redo to finish before retrying files");
            return;
        }
        let record = claim.record();
        let kind = record.kind;
        let sources = record.sources.clone();
        let destination = record.destination.clone().unwrap();
        let retry = match kind {
            JobKind::Copy => OperationRetry::Copy {
                pane,
                sources,
                destination,
            },
            JobKind::Move => OperationRetry::Move {
                pane,
                sources,
                destination,
            },
            _ => return,
        };
        let options = TransferOptions {
            conflict_policy: ConflictPolicy::Ask,
            verify: true,
            durable: true,
            parallel: self.parallel_transfers,
        };
        match self.operation_engine.spawn_recovered(*claim, options) {
            Ok(handle) => {
                self.monitor_operation(pane, kind, handle, None, retry, sender);
            }
            Err(error) => notifications::error(&error),
        }
    }
}

#[cfg(test)]
mod tests;
