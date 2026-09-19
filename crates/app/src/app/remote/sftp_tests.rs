use super::*;

#[test]
#[ignore = "requires the isolated OpenSSH/GVfs fixture from scripts/test-sftp.py"]
fn sftp_move_browse_copy_back_and_delete_preserve_contents() {
    let uri = std::env::var("COMMANDER_SFTP_TEST_URI").expect("loopback SFTP fixture");
    assert!(uri.contains("@127.0.0.1:"));
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
