use super::*;

fn operation(state: JobState) -> OperationStatus {
    OperationStatus {
        phase: JobPhase::Copying,
        kind: OperationKind::Files(JobKind::Copy),
        state,
        progress: JobProgress {
            bytes_done: 256 * 1024,
            bytes_total: 1024 * 1024,
            items_done: 1,
            items_total: 4,
            current_path: Some(VPath::from("/source/report.pdf")),
            throughput_bytes_per_second: 128.0 * 1024.0,
            eta: Some(Duration::from_secs(6)),
        },
        control: JobControl::new(),
        commands: std::sync::mpsc::channel().0,
        retry: OperationRetry::Copy {
            pane: PaneId::Left,
            sources: vec![VPath::from("/source/report.pdf")],
            destination: VPath::from("/destination"),
        },
        waiting_for_conflict: false,
        error: None,
    }
}

#[test]
fn preparing_has_no_invented_percentage() {
    let mut operation = operation(JobState::Scanning);
    operation.progress = JobProgress::default();
    let view = JobPresentation::new(&operation);
    assert_eq!(view.fraction, None);
    assert_eq!(view.status, "Preparing…");
    assert!(view.busy);
    assert_eq!(view.path, "/source/report.pdf → /destination");
}

#[test]
fn archives_show_activity_counts_paths_and_terminal_states() {
    let mut operation = operation(JobState::Running);
    operation.kind = OperationKind::CreateArchive;
    operation.progress.bytes_total = 0;
    operation.progress.items_total = 0;
    operation.retry = OperationRetry::Archive {
        pane: PaneId::Left,
        sources: vec![VPath::from("/source/report.pdf")],
        destination: VPath::from("/destination/backup.zip"),
        format: Some(ArchiveFormat::Zip),
        password: None,
    };
    let view = JobPresentation::new(&operation);
    assert_eq!(view.title, "Creating archive");
    assert_eq!(view.fraction, None);
    assert!(view.busy && view.can_cancel && view.can_pause);
    assert!(view.detail.contains("processed"));
    assert!(!view.detail.contains("remaining"));
    assert_eq!(view.path, "/source/report.pdf → /destination/backup.zip");
    operation.phase = JobPhase::Finishing;
    assert_eq!(JobPresentation::new(&operation).status, "Finishing…");
    operation.state = JobState::Done;
    assert_eq!(JobPresentation::new(&operation).status, "Completed");
    operation.state = JobState::Cancelled;
    assert!(JobPresentation::new(&operation).can_retry);
    operation.kind = OperationKind::ExtractArchive;
    assert!(!JobPresentation::new(&operation).can_retry);
}

#[test]
fn progress_reports_bytes_speed_eta_and_finishing_separately() {
    let mut operation = operation(JobState::Running);
    let view = JobPresentation::new(&operation);
    assert_eq!(view.fraction, Some(0.25));
    assert_eq!(view.title, "Copying");
    assert!(view.detail.contains("/s"));
    assert!(view.detail.ends_with("6s remaining"));
    operation.progress.bytes_done = operation.progress.bytes_total;
    let view = JobPresentation::new(&operation);
    assert_eq!(view.status, "Finishing…");
    assert!(view.busy && !view.finished);
    assert!(!view.detail.contains("remaining"));
    operation.state = JobState::Done;
    let view = JobPresentation::new(&operation);
    assert_eq!(view.status, "Completed");
    assert!(view.finished && !view.busy);
}

#[test]
fn trash_delete_and_empty_files_use_item_progress() {
    for kind in [JobKind::Trash, JobKind::DeletePermanent, JobKind::Copy] {
        let mut operation = operation(JobState::Running);
        operation.kind = OperationKind::Files(kind);
        operation.progress.bytes_done = 0;
        if kind == JobKind::Copy {
            operation.progress.bytes_total = 0;
        }
        let view = JobPresentation::new(&operation);
        assert_eq!(view.fraction, Some(0.25));
        assert_eq!(view.detail, "1 / 4 items");
    }
}

#[test]
fn secure_delete_has_progress_and_controls_but_never_a_blind_retry() {
    let mut job = operation(JobState::Running);
    job.kind = OperationKind::SecureDelete;
    job.retry = OperationRetry::SecureDelete {
        sources: vec![VPath::from("/source/report.pdf")],
    };
    let view = JobPresentation::new(&job);
    assert_eq!(view.title, "Overwriting files");
    assert_eq!(view.fraction, Some(0.25));
    assert!(view.can_cancel && view.can_pause);
    job.phase = JobPhase::Verifying;
    assert_eq!(JobPresentation::new(&job).status, "Verifying…");
    for state in [JobState::Cancelled, JobState::Failed] {
        job.state = state;
        assert!(!JobPresentation::new(&job).can_retry);
    }
}

#[test]
fn pause_cancel_and_conflicts_do_not_show_stale_speed_or_eta() {
    let mut operation = operation(JobState::Scanning);
    operation.control.pause();
    // A queued engine transition must not erase the user's pause request.
    operation.state = JobState::Running;
    let view = JobPresentation::new(&operation);
    assert_eq!(view.status, "Paused");
    assert!(view.paused && view.can_pause);
    assert!(!view.busy && !view.detail.contains("remaining"));
    assert!(!view.detail.contains("/s"));
    operation.control.resume();
    operation.waiting_for_conflict = true;
    let view = JobPresentation::new(&operation);
    assert_eq!(view.status, "Needs a decision");
    assert!(!view.busy && !view.can_pause && view.can_cancel);
    operation.control.cancel();
    let view = JobPresentation::new(&operation);
    assert_eq!(view.status, "Cancelling…");
    assert!(!view.can_cancel && !view.can_pause && !view.finished);
}

#[test]
fn failed_and_cancelled_jobs_keep_progress_and_allow_retry() {
    for state in [JobState::Failed, JobState::Cancelled] {
        let mut operation = operation(state);
        operation.error = Some("Disk full".to_owned());
        let view = JobPresentation::new(&operation);
        assert_eq!(view.fraction, Some(0.25));
        assert_eq!(view.error.as_deref(), Some("Disk full"));
        assert!(view.can_retry && view.finished);
        assert!(!view.can_pause && !view.can_cancel && !view.busy);
    }
}

#[test]
fn active_jobs_take_precedence_over_completed_history_and_cancelling_jobs() {
    let old = JobId::next();
    let active = JobId::next();
    let recent = JobId::next();
    let mut operations = BTreeMap::from([
        (old, operation(JobState::Done)),
        (active, operation(JobState::Running)),
        (recent, operation(JobState::Done)),
    ]);
    assert_eq!(featured_operation(&operations), Some(active));
    assert_eq!(first_active_operation(&operations), Some(active));
    operations[&active].control.cancel();
    assert_eq!(first_active_operation(&operations), None);
    assert_eq!(featured_operation(&operations), Some(active));
    operations.get_mut(&active).unwrap().state = JobState::Cancelled;
    assert_eq!(featured_operation(&operations), Some(recent));
    operations.clear();
    assert_eq!(featured_operation(&operations), None);
}

#[test]
fn incomplete_metrics_are_bounded_and_never_render_nan_or_infinity() {
    let mut operation = operation(JobState::Running);
    operation.progress.bytes_done = u64::MAX;
    assert_eq!(JobPresentation::new(&operation).fraction, Some(1.0));
    operation.progress.bytes_done = 1;
    for rate in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
        operation.progress.throughput_bytes_per_second = rate;
        assert!(!JobPresentation::new(&operation).detail.contains("/s"));
    }
    operation.progress.bytes_total = 0;
    operation.progress.items_total = 0;
    assert_eq!(JobPresentation::new(&operation).fraction, None);
    operation.state = JobState::Done;
    assert_eq!(JobPresentation::new(&operation).fraction, Some(1.0));
}

#[test]
#[ignore = "requires a GTK display; run alone with --ignored --test-threads=1"]
fn gtk_activity_controls_and_rows_survive_progress_updates() {
    adw::init().unwrap();
    crate::icons::install(&gdk::Display::default().unwrap());
    install_styles(AppearanceMode::Dark);
    let (input, receiver) = relm4::channel();
    let widgets = JobActivityWidgets::new(&input);
    let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let files = gtk::Box::new(gtk::Orientation::Vertical, 0);
    files.set_vexpand(true);
    shell.append(&files);
    shell.append(&widgets.root);
    let window = gtk::Window::builder()
        .title("Commander job indicator test")
        .default_width(700)
        .default_height(150)
        .child(&shell)
        .build();
    let mut operations = BTreeMap::new();
    widgets.render(&operations, &input);
    assert!(!widgets.root.is_visible());
    let id = JobId::next();
    operations.insert(id, operation(JobState::Running));
    widgets.render(&operations, &input);
    window.present();
    drain_frames();
    assert!(widgets.root.is_visible());
    assert_eq!(widgets.progress.fraction(), 0.25);
    assert!(!widgets.spinner.is_spinning());
    assert!(widgets.title.tooltip_text().unwrap().contains("remaining"));
    let row_before = widgets.list.rows.borrow()[&id].root.clone();
    operations.get_mut(&id).unwrap().progress.bytes_done *= 2;
    widgets.render(&operations, &input);
    assert_eq!(widgets.list.rows.borrow()[&id].root, row_before);
    assert_eq!(widgets.progress.fraction(), 0.5);
    widgets.pause.emit_clicked();
    assert!(matches!(receiver.recv_sync(), Some(AppMsg::TogglePauseOperation(job)) if job == id));
    operations[&id].control.pause();
    widgets.render(&operations, &input);
    assert_eq!(
        widgets.pause.tooltip_text().as_deref(),
        Some("Resume operation")
    );
    assert!(!widgets.spinner.is_spinning());
    assert_eq!(
        widgets.list.rows.borrow()[&id].pause.icon_name().as_deref(),
        Some("commander-play-symbolic")
    );
    assert_eq!(
        widgets.list.rows.borrow()[&id]
            .pause
            .tooltip_text()
            .as_deref(),
        Some("Resume operation")
    );
    widgets.list.rows.borrow()[&id].pause.emit_clicked();
    assert!(matches!(receiver.recv_sync(), Some(AppMsg::TogglePauseOperation(job)) if job == id));
    operations[&id].control.resume();
    widgets.render(&operations, &input);
    widgets.menu.popup();
    drain_frames();
    assert!(
        widgets.root.width() <= 700,
        "activity strip widened the window"
    );
    assert!(
        widgets.root.height() <= 34,
        "activity strip must fit on a single compact row"
    );
    assert!(widgets.root.measure(gtk::Orientation::Horizontal, -1).0 <= 360);
    assert!(
        widgets.progress.width() <= 90,
        "progress stays short: {}px",
        widgets.progress.width()
    );
    snapshot(&widgets.root, "running");
    let popover = widgets.menu.popover().unwrap();
    snapshot(&popover.child().unwrap(), "jobs");
    apply_appearance(AppearanceMode::Light);
    drain_frames();
    snapshot(&widgets.root, "running-light");
    snapshot(&popover.child().unwrap(), "jobs-light");
    apply_appearance(AppearanceMode::Dark);
    widgets.menu.popdown();
    operations.get_mut(&id).unwrap().state = JobState::Done;
    widgets.render(&operations, &input);
    assert!(!widgets.progress.is_visible());
    assert!(!widgets.spinner.is_spinning());
    assert_eq!(
        widgets.state_icon.icon_name().as_deref(),
        Some("commander-circle-check-symbolic")
    );
    drain_frames();
    snapshot(&widgets.root, "completed");
    operations.get_mut(&id).unwrap().state = JobState::Running;
    widgets.render(&operations, &input);
    widgets.cancel.emit_clicked();
    assert!(matches!(receiver.recv_sync(), Some(AppMsg::CancelOperation(job)) if job == id));
    operations[&id].control.cancel();
    widgets.render(&operations, &input);
    assert!(!widgets.cancel.is_sensitive());
    let failed = operations.get_mut(&id).unwrap();
    failed.state = JobState::Failed;
    failed.error = Some("Could not write report.pdf: No space left on device".to_owned());
    widgets.render(&operations, &input);
    assert!(widgets.dismiss.is_visible());
    assert!(!widgets.cancel.is_visible());
    assert!(!widgets.progress.is_visible());
    assert!(widgets.root.has_css_class("job-activity-attention"));
    assert!(
        widgets
            .title
            .tooltip_text()
            .unwrap()
            .contains("No space left")
    );
    widgets.list.rows.borrow()[&id].retry.emit_clicked();
    assert!(matches!(receiver.recv_sync(), Some(AppMsg::RetryOperation(job)) if job == id));
    drain_frames();
    snapshot(&widgets.root, "failed");
    widgets.dismiss.emit_clicked();
    assert!(matches!(receiver.recv_sync(), Some(AppMsg::DismissOperation(job)) if job == id));
    operations.clear();
    widgets.render(&operations, &input);
    assert!(!widgets.root.is_visible());
    assert!(widgets.list.rows.borrow().is_empty());
    let mut archive = operation(JobState::Running);
    archive.kind = OperationKind::CreateArchive;
    archive.progress.bytes_total = 0;
    archive.progress.items_total = 0;
    archive.progress.bytes_done = 935_120_076;
    archive.progress.items_done = 1135;
    archive.retry = OperationRetry::Archive {
        pane: PaneId::Left,
        sources: vec![VPath::from("/source/report.pdf")],
        destination: VPath::from("/destination/backup.zip"),
        format: Some(ArchiveFormat::Zip),
        password: None,
    };
    operations.insert(id, archive);
    widgets.render(&operations, &input);
    drain_frames();
    assert!(widgets.root.is_visible() && widgets.spinner.is_spinning());
    assert_eq!(widgets.title.label(), "Creating archive");
    assert!(!widgets.progress.is_visible());
    snapshot(&widgets.root, "archive-running");
    widgets.menu.popup();
    drain_frames();
    snapshot(
        &widgets.menu.popover().unwrap().child().unwrap(),
        "archive-jobs",
    );
    widgets.menu.popdown();
    // Inspect the same job card at the narrow width used by the inspector.
    let compact = JobListWidgets::new();
    compact.root.add_css_class("inspector-page");
    compact.render(&operations, &input);
    let compact_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(246)
        .max_content_width(246)
        .propagate_natural_width(false)
        .propagate_natural_height(true)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .child(&compact.root)
        .build();
    let compact_window = gtk::Window::builder()
        .title("Commander compact job cards")
        .default_width(246)
        .child(&compact_scroll)
        .build();
    compact_window.present();
    drain_frames();
    assert_eq!(compact_scroll.width(), 246);
    assert!(compact.root.width() <= 246);
    snapshot(&compact.root, "archive-card-narrow");
    apply_appearance(AppearanceMode::Light);
    drain_frames();
    snapshot(&compact.root, "archive-card-narrow-light");
    apply_appearance(AppearanceMode::Dark);
    let another = JobId::next();
    operations.insert(another, operation(JobState::Running));
    compact.render(&operations, &input);
    drain_frames();
    snapshot(&compact.root, "multiple-cards-narrow");
    operations.remove(&another);
    for (state, message, name) in [
        (
            JobState::Cancelled,
            "Archive operation cancelled",
            "cancelled-card",
        ),
        (
            JobState::Failed,
            "Not enough space to finish the archive",
            "failed-card",
        ),
    ] {
        let job = operations.get_mut(&id).unwrap();
        job.state = state;
        job.error = Some(message.to_owned());
        compact.render(&operations, &input);
        drain_frames();
        snapshot(&compact.root, name);
        apply_appearance(AppearanceMode::Light);
        drain_frames();
        snapshot(&compact.root, &format!("{name}-light"));
        apply_appearance(AppearanceMode::Dark);
    }
    let job = operations.get_mut(&id).unwrap();
    job.state = JobState::Running;
    job.error = None;
    compact_window.close();
    operations[&id].control.pause();
    widgets.render(&operations, &input);
    assert!(!widgets.spinner.is_spinning());
    assert!(widgets.title.label().contains("Paused"));
    drain_frames();
    snapshot(&widgets.root, "archive-paused");
    widgets.list.rows.borrow()[&id].cancel.emit_clicked();
    assert!(matches!(receiver.recv_sync(), Some(AppMsg::CancelOperation(job)) if job == id));
    operations.clear();
    widgets.render(&operations, &input);
    window.close();
}

fn drain_frames() {
    let context = glib::MainContext::default();
    let until = Instant::now() + Duration::from_millis(200);
    while Instant::now() < until {
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn snapshot(widget: &impl IsA<gtk::Widget>, name: &str) {
    let Some(directory) = std::env::var_os("COMMANDER_JOB_SNAPSHOT_DIR") else {
        return;
    };
    let widget = widget.as_ref();
    assert!(
        widget.width() > 0 && widget.height() > 0,
        "{name} is allocated"
    );
    let snapshot = gtk::Snapshot::new();
    let parent = widget.parent().expect("snapshot a child widget");
    parent.snapshot_child(widget, &snapshot);
    let node = snapshot
        .to_node()
        .expect("visible widget has a render node");
    let texture = widget
        .native()
        .unwrap()
        .renderer()
        .unwrap()
        .render_texture(&node, widget.compute_bounds(&parent).as_ref());
    texture
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}

#[test]
fn verification_has_its_own_phase_without_copy_eta() {
    let mut job = operation(JobState::Running);
    job.phase = JobPhase::Verifying;
    job.progress.bytes_total = 100;
    job.progress.bytes_done = 100;
    job.progress.throughput_bytes_per_second = 10_000.0;
    job.progress.eta = Some(Duration::from_secs(2));
    let view = JobPresentation::new(&job);
    assert_eq!(view.status, "Verifying…");
    assert!(!view.detail.contains("remaining"));
    assert!(view.can_pause && view.can_cancel);
}
