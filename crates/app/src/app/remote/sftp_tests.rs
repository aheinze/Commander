use super::*;

#[test]
#[ignore = "requires the isolated OpenSSH/GVfs fixture from scripts/test-sftp.py"]
fn sftp_move_browse_copy_back_and_delete_preserve_contents() {
    let uri = std::env::var("COMMANDER_SFTP_TEST_URI").expect("loopback SFTP fixture");
    assert!(
        uri.contains("@127.0.0.1:")
            || (std::env::var("COMMANDER_SFTP_REAL_SERVER").as_deref() == Ok("1")
                && uri.contains("/tmp/commander-sftp-")),
        "live-server checks require an explicitly allocated disposable directory"
    );
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            context.block_on(async {
                let connection = RemoteConnection::parse(&uri).unwrap();
                let file = gio::File::for_uri(&connection.uri);
                let mounted = mount_connection(connection).await.unwrap();
                assert!(
                    !gio::File::for_path(mounted.as_path()).is_native(),
                    "remote watch/delete detection"
                );
                let fixture = tempfile::tempdir().unwrap();
                let source = fixture.path().join("preview\\.agentejo.work");
                std::fs::create_dir_all(source.join("nested folder")).unwrap();
                let payload = vec![b'N'; 128 * 1024];
                let first = source.join("nested folder/first.txt");
                std::fs::write(&first, &payload).unwrap();
                std::fs::hard_link(&first, source.join("nested folder/linked.txt")).unwrap();
                std::fs::File::open(&first)
                    .unwrap()
                    .set_modified(std::time::UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789))
                    .unwrap();
                let engine =
                    OperationEngine::default().with_journal_directory(fixture.path().join("jobs"));
                let options = TransferOptions {
                    conflict_policy: ConflictPolicy::Overwrite,
                    verify: true,
                    durable: true,
                    parallel: false,
                };
                let moved = engine
                    .spawn_move(
                        vec![VPath::from(source.as_path())],
                        mounted.clone(),
                        ScanOptions::default(),
                        options,
                    )
                    .join();
                assert_eq!(moved.state, JobState::Done, "{moved:?}");
                assert!(!source.exists());
                let remote = mounted.join_name(source.file_name().unwrap());
                let nested = remote.join_name(OsStr::new("nested folder"));
                let listing =
                    ListingTask::spawn(Arc::new(LocalFs), ListingRequest::new(nested.clone()))
                        .unwrap()
                        .wait_complete()
                        .unwrap()
                        .0;
                assert_eq!(listing.len(), 2);
                for entry in listing.rows() {
                    assert_eq!(
                        std::fs::read(nested.join_name(entry.name()).as_path()).unwrap(),
                        payload
                    );
                }
                let download = fixture.path().join("download");
                std::fs::create_dir(&download).unwrap();
                let copied = engine
                    .spawn_copy(
                        vec![remote.clone()],
                        VPath::from(download.as_path()),
                        ScanOptions::default(),
                        options,
                    )
                    .join();
                assert_eq!(copied.state, JobState::Done, "{copied:?}");
                for name in ["first.txt", "linked.txt"] {
                    assert_eq!(
                        std::fs::read(
                            download
                                .join(source.file_name().unwrap())
                                .join("nested folder")
                                .join(name)
                        )
                        .unwrap(),
                        payload
                    );
                }
                let deleted = engine
                    .spawn_delete_permanently(vec![remote.clone()], 1)
                    .join();
                assert_eq!(deleted.state, JobState::Done, "{deleted:?}");
                assert!(!remote.as_path().exists());
                assert_eq!(std::fs::read_dir(mounted.as_path()).unwrap().count(), 0);
                file.find_enclosing_mount(gio::Cancellable::NONE)
                    .unwrap()
                    .unmount_with_operation_future(
                        gio::MountUnmountFlags::FORCE,
                        gio::MountOperation::NONE,
                    )
                    .await
                    .unwrap();
            })
        })
        .unwrap();
}

fn fault(mode: &str, rate: u64) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(1);
    assert!(
        std::env::var("COMMANDER_SFTP_TEST_URI")
            .unwrap()
            .contains("@127.0.0.1:")
    );
    let fixture = std::path::PathBuf::from(std::env::var_os("COMMANDER_SFTP_CONTROL").unwrap());
    let serial = format!("rust-{}", SERIAL.fetch_add(1, Ordering::SeqCst));
    let request = serde_json::json!({ "serial": serial, "mode": mode, "rate": rate });
    std::fs::write(fixture.join("control.tmp"), request.to_string()).unwrap();
    std::fs::rename(fixture.join("control.tmp"), fixture.join("control.json")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(bytes) = std::fs::read(fixture.join("ack.json"))
            && let Ok(ack) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && ack["serial"] == serial
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fault proxy did not acknowledge {mode}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn backing() -> std::path::PathBuf {
    assert!(
        std::env::var("COMMANDER_SFTP_TEST_URI")
            .unwrap()
            .contains("@127.0.0.1:")
    );
    std::env::var_os("COMMANDER_SFTP_ROOT").unwrap().into()
}

fn transfer_options() -> TransferOptions {
    TransferOptions {
        conflict_policy: ConflictPolicy::Overwrite,
        verify: true,
        durable: true,
        parallel: false,
    }
}

fn finish(job: JobHandle, mut event: impl FnMut(&JobEvent)) -> dualpane_engine::JobSummary {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "SFTP operation did not finish");
        match job.events().recv_timeout(Duration::from_millis(50)) {
            Ok(value) => {
                event(&value);
                if matches!(value, JobEvent::Finished { .. }) {
                    break;
                }
                if matches!(value, JobEvent::Conflict { .. }) {
                    job.cancel();
                    panic!("unexpected conflict: {value:?}");
                }
            }
            Err(error) if error.is_timeout() => {}
            Err(error) => panic!("SFTP job ended without a summary: {error}"),
        }
    }
    job.join()
}

async fn mount_test() -> VPath {
    let uri = std::env::var("COMMANDER_SFTP_TEST_URI").unwrap();
    mount_connection(RemoteConnection::parse(&uri).unwrap())
        .await
        .unwrap()
}

async fn unmount_test() {
    let file = gio::File::for_uri(&std::env::var("COMMANDER_SFTP_TEST_URI").unwrap());
    if let Ok(mount) = file.find_enclosing_mount(gio::Cancellable::NONE) {
        let _ = mount
            .unmount_with_operation_future(gio::MountUnmountFlags::FORCE, gio::MountOperation::NONE)
            .await;
    }
}

#[test]
#[ignore = "requires the isolated OpenSSH/GVfs fault fixture from scripts/test-sftp.py"]
fn sftp_disconnect_during_upload_preserves_sources_and_retries_after_reconnect() {
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            context.block_on(async {
                let target = mount_test().await;
                let local = tempfile::tempdir().unwrap();
                let good = local.path().join("upload-completed.bin");
                let large = local.path().join("upload-interrupted.bin");
                let payload = vec![0xA7; 16 * 1024 * 1024];
                std::fs::write(&good, b"completed upload").unwrap();
                std::fs::write(&large, &payload).unwrap();
                std::fs::write(
                    backing().join(large.file_name().unwrap()),
                    b"original remote destination",
                )
                .unwrap();
                let engine =
                    OperationEngine::default().with_journal_directory(local.path().join("jobs"));
                fault("forward", 512 * 1024);
                let mut disconnected = false;
                let first = finish(
                    engine.spawn_move(
                        vec![good.clone().into(), large.clone().into()],
                        target.clone(),
                        ScanOptions::default(),
                        transfer_options(),
                    ),
                    |event| {
                        if let JobEvent::Progress { progress, .. } = event
                            && progress.current_path.as_ref() == Some(&large.clone().into())
                            && progress.bytes_done > 64 * 1024
                            && !disconnected
                        {
                            fault("offline", 0);
                            disconnected = true;
                        }
                    },
                );
                assert!(disconnected, "fault must interrupt a real transfer");
                assert_eq!(first.state, JobState::Failed, "{first:?}");
                assert!(
                    !good.exists(),
                    "the completed move should be recorded before the failure"
                );
                assert_eq!(std::fs::read(&large).unwrap(), payload);
                assert_eq!(
                    std::fs::read(backing().join(large.file_name().unwrap())).unwrap(),
                    b"original remote destination"
                );
                assert_eq!(
                    std::fs::read(backing().join(good.file_name().unwrap())).unwrap(),
                    b"completed upload"
                );
                unmount_test().await;
                fault("forward", 0);
                let reconnected = mount_test().await;
                assert_eq!(
                    target, reconnected,
                    "reconnection must restore saved native paths"
                );
                let plan = dualpane_engine::recovery::RecoveryPlan::load(
                    first.outcome.journal_path.as_ref().unwrap(),
                )
                .unwrap();
                let retried = finish(
                    engine
                        .spawn_recovered(plan.claim().unwrap(), transfer_options())
                        .unwrap(),
                    |_| {},
                );
                assert_eq!(retried.state, JobState::Done, "{retried:?}");
                assert!(!large.exists());
                assert_eq!(
                    std::fs::read(backing().join(large.file_name().unwrap())).unwrap(),
                    payload
                );
                assert!(
                    dualpane_engine::journal::read_journal(
                        first.outcome.journal_path.as_ref().unwrap()
                    )
                    .unwrap()
                    .retry
                    .is_some()
                );
                unmount_test().await;
            })
        })
        .unwrap();
}

#[test]
#[ignore = "requires the isolated OpenSSH/GVfs fault fixture from scripts/test-sftp.py"]
fn sftp_disconnect_during_download_preserves_original_and_retries_after_reconnect() {
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            context.block_on(async {
                let local = tempfile::tempdir().unwrap();
                let payload = vec![0xB6; 16 * 1024 * 1024];
                std::fs::write(
                    backing().join("download-completed.bin"),
                    b"completed download",
                )
                .unwrap();
                std::fs::write(backing().join("download-interrupted.bin"), &payload).unwrap();
                std::fs::write(
                    local.path().join("download-interrupted.bin"),
                    b"original local destination",
                )
                .unwrap();
                let remote = mount_test().await;
                let good = remote.join_name(OsStr::new("download-completed.bin"));
                let large = remote.join_name(OsStr::new("download-interrupted.bin"));
                let engine =
                    OperationEngine::default().with_journal_directory(local.path().join("jobs"));
                fault("forward", 512 * 1024);
                let mut disconnected = false;
                let first = finish(
                    engine.spawn_copy(
                        vec![good, large.clone()],
                        local.path().into(),
                        ScanOptions::default(),
                        transfer_options(),
                    ),
                    |event| {
                        if let JobEvent::Progress { progress, .. } = event
                            && progress.current_path.as_ref() == Some(&large)
                            && progress.bytes_done > 64 * 1024
                            && !disconnected
                        {
                            fault("offline", 0);
                            disconnected = true;
                        }
                    },
                );
                assert!(disconnected);
                assert_eq!(first.state, JobState::Failed, "{first:?}");
                assert_eq!(
                    std::fs::read(local.path().join("download-interrupted.bin")).unwrap(),
                    b"original local destination"
                );
                let completed: VPath = local.path().join("download-completed.bin").into();
                let identity = LocalFs.stat(&completed, false).unwrap().identity;
                unmount_test().await;
                fault("forward", 0);
                assert_eq!(remote, mount_test().await);
                let plan = dualpane_engine::recovery::RecoveryPlan::load(
                    first.outcome.journal_path.as_ref().unwrap(),
                )
                .unwrap();
                let retry = finish(
                    engine
                        .spawn_recovered(plan.claim().unwrap(), transfer_options())
                        .unwrap(),
                    |_| {},
                );
                assert_eq!(retry.state, JobState::Done, "{retry:?}");
                assert_eq!(
                    std::fs::read(local.path().join("download-interrupted.bin")).unwrap(),
                    payload
                );
                assert_eq!(
                    LocalFs.stat(&completed, false).unwrap().identity,
                    identity,
                    "completed downloads must be reused"
                );
                assert_eq!(
                    std::fs::read(backing().join("download-interrupted.bin")).unwrap(),
                    payload
                );
                unmount_test().await;
            })
        })
        .unwrap();
}

#[test]
#[ignore = "requires the isolated OpenSSH/GVfs fault fixture from scripts/test-sftp.py"]
fn sftp_stalled_listing_times_out_and_cancels_without_waiting_for_io() {
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            context.block_on(async {
                let remote = mount_test().await;
                for name in ["stalled-listing", "cancelled-listing"] {
                    std::fs::create_dir(backing().join(name)).unwrap();
                    std::fs::write(backing().join(name).join("proof"), b"listing recovers")
                        .unwrap();
                }
                fault("paused", 0);
                let started = Instant::now();
                let task = ListingTask::spawn(
                    Arc::new(LocalFs),
                    ListingRequest::new(remote.join_name(OsStr::new("stalled-listing"))),
                )
                .unwrap();
                let result = task.wait_complete();
                assert!(
                    matches!(
                        result,
                        Err(dualpane_index::IndexError::ListingTimeout { .. })
                    ),
                    "{result:?}"
                );
                assert!(
                    started.elapsed() >= Duration::from_secs(29)
                        && started.elapsed() < Duration::from_secs(40)
                );
                drop(task); // Must not join the blocked GVfs call.
                let task = ListingTask::spawn(
                    Arc::new(LocalFs),
                    ListingRequest::new(remote.join_name(OsStr::new("cancelled-listing"))),
                )
                .unwrap();
                thread::sleep(Duration::from_millis(150));
                let started = Instant::now();
                task.cancel();
                assert!(matches!(
                    task.wait_complete(),
                    Err(dualpane_index::IndexError::Cancelled)
                ));
                drop(task);
                assert!(started.elapsed() < Duration::from_secs(1));
                fault("forward", 0);
                let listing = ListingTask::spawn(
                    Arc::new(LocalFs),
                    ListingRequest::new(remote.join_name(OsStr::new("stalled-listing"))),
                )
                .unwrap()
                .wait_complete()
                .unwrap()
                .0;
                assert_eq!(listing.len(), 1);
                unmount_test().await;
            })
        })
        .unwrap();
}

#[test]
#[ignore = "requires the isolated OpenSSH/GVfs fault fixture from scripts/test-sftp.py"]
fn sftp_stalled_transfer_cancel_keeps_the_source_and_original() {
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            context.block_on(async {
                let remote = mount_test().await;
                let local = tempfile::tempdir().unwrap();
                let source = local.path().join("cancel-upload.bin");
                let payload = vec![0xC5; 16 * 1024 * 1024];
                std::fs::write(&source, &payload).unwrap();
                std::fs::write(backing().join("cancel-upload.bin"), b"keep original").unwrap();
                let engine =
                    OperationEngine::default().with_journal_directory(local.path().join("jobs"));
                fault("forward", 512 * 1024);
                let job = engine.spawn_move(
                    vec![source.clone().into()],
                    remote,
                    ScanOptions::default(),
                    transfer_options(),
                );
                let control = job.control();
                let mut interrupted = false;
                let result = finish(job, |event| {
                    if let JobEvent::Progress { progress, .. } = event
                        && progress.bytes_done > 64 * 1024
                        && !interrupted
                    {
                        fault("paused", 0);
                        thread::sleep(Duration::from_secs(2));
                        control.cancel();
                        // Cancellation must prevent publication when the blocked I/O returns.
                        fault("forward", 0);
                        interrupted = true;
                    }
                });
                assert!(interrupted);
                assert_eq!(result.state, JobState::Cancelled, "{result:?}");
                assert_eq!(std::fs::read(source).unwrap(), payload);
                assert_eq!(
                    std::fs::read(backing().join("cancel-upload.bin")).unwrap(),
                    b"keep original"
                );
                unmount_test().await;
            })
        })
        .unwrap();
}

#[test]
#[ignore = "requires an explicitly requested real-server read-only directory check"]
fn sftp_existing_folder_is_readable() {
    assert_eq!(
        std::env::var("COMMANDER_SFTP_REAL_SERVER").as_deref(),
        Ok("1")
    );
    let uri = std::env::var("COMMANDER_SFTP_BROWSE_URI").unwrap();
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            context.block_on(async {
                let path = mount_connection(RemoteConnection::parse(&uri).unwrap())
                    .await
                    .unwrap();
                let started = Instant::now();
                let listing = ListingTask::spawn(Arc::new(LocalFs), ListingRequest::new(path))
                    .unwrap()
                    .wait_complete()
                    .unwrap()
                    .0;
                eprintln!(
                    "Read-only server listing: {} entries in {:?}",
                    listing.len(),
                    started.elapsed()
                );
                unmount_test().await;
            })
        })
        .unwrap();
}
