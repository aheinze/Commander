use super::*;
use dualpane_vfs::LocalFs;
use std::fs;

const SECRET: &str = "  synthetic archive password 🔑  ";
fn secret() -> Option<Password> {
    Password::new(SECRET.into())
}

fn make(fixture: &Path, format: ArchiveFormat) -> VPath {
    let source = fixture.join("private");
    fs::create_dir_all(source.join("empty folder")).unwrap();
    fs::write(source.join("notes.txt"), "private contents").unwrap();
    fs::write(source.join("empty.txt"), "").unwrap();
    let archive = VPath::from(fixture.join(format!("bundle.{}", format.extension())));
    let mut task = ArchiveTask::new(&CancelToken::new());
    task.set_password(secret());
    create_archive(&LocalFs, &[source.into()], &archive, format, &mut task).unwrap();
    archive
}

#[test]
fn encrypted_archive_round_trip_retry_edit_and_undo_preserve_encryption() {
    for format in [ArchiveFormat::Zip, ArchiveFormat::SevenZ] {
        let fixture = tempfile::tempdir().unwrap();
        let source = make(fixture.path(), format);
        let original = fs::read(source.as_path()).unwrap();
        let output = tempfile::tempdir().unwrap();
        let destination = VPath::from(output.path());
        let mut task = ArchiveTask::new(&CancelToken::new());
        assert!(
            extract_archive(&LocalFs, &source, &destination, &mut task)
                .unwrap_err()
                .contains("requires a password")
        );
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
        task.set_password(Password::new("wrong".into()));
        assert!(extract_archive(&LocalFs, &source, &destination, &mut task).is_err());
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
        let mut attempts = Vec::new();
        let mut task = ArchiveTask::new(&CancelToken::new());
        task.set_password_prompt(|requested, incorrect| {
            assert_eq!(requested, &source);
            assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
            attempts.push(incorrect);
            if incorrect {
                secret()
            } else {
                Password::new("wrong".into())
            }
        });
        extract_archive(&LocalFs, &source, &destination, &mut task).unwrap();
        drop(task);
        assert_eq!(attempts, [false, true]);
        assert_eq!(
            fs::read_to_string(output.path().join("private/notes.txt")).unwrap(),
            "private contents"
        );
        assert!(output.path().join("private/empty folder").is_dir());
        assert_eq!(
            fs::metadata(output.path().join("private/empty.txt"))
                .unwrap()
                .len(),
            0
        );
        let imported = fixture.path().join("added.txt");
        fs::write(&imported, "new secret contents").unwrap();
        let snapshot = edit::inspect_with_password(&source, &CancelToken::new(), secret()).unwrap();
        assert!(!format!("{snapshot:?}").contains(SECRET));
        let mut changes = edit::Changes::default();
        changes
            .renamed
            .insert("private/notes.txt".into(), "private/renamed.txt".into());
        changes.added.insert("private/added.txt".into(), imported);
        changes.removed.insert("private/empty.txt".into());
        let change = edit::save_with_task(
            &snapshot,
            &changes,
            &mut ArchiveTask::new(&CancelToken::new()),
        )
        .unwrap();
        assert!(encrypted(&LocalFs, &source, &ArchiveTask::new(&CancelToken::new())).unwrap());
        let updated = tempfile::tempdir().unwrap();
        let mut task = ArchiveTask::new(&CancelToken::new());
        task.set_password(secret());
        extract_archive(&LocalFs, &source, &updated.path().into(), &mut task).unwrap();
        assert_eq!(
            fs::read_to_string(updated.path().join("private/renamed.txt")).unwrap(),
            "private contents"
        );
        assert_eq!(
            fs::read_to_string(updated.path().join("private/added.txt")).unwrap(),
            "new secret contents"
        );
        assert!(!updated.path().join("private/empty.txt").exists());
        assert!(updated.path().join("private/empty folder").is_dir());
        if format == ArchiveFormat::Zip {
            let mut zip = ZipArchive::new(fs::File::open(source.as_path()).unwrap()).unwrap();
            for index in 0..zip.len() {
                let entry = zip.by_index_raw(index).unwrap();
                assert!(entry.is_dir() || entry.encrypted());
            }
        } else {
            assert!(matches!(
                SevenZReader::open(source.as_path(), SevenZPassword::empty()),
                Err(sevenz_rust2::Error::PasswordRequired)
            ));
        }
        let reverse = edit::restore(&change, &mut ArchiveTask::new(&CancelToken::new())).unwrap();
        assert_eq!(fs::read(source.as_path()).unwrap(), original);
        edit::restore(&reverse, &mut ArchiveTask::new(&CancelToken::new())).unwrap();
        validate(
            &LocalFs,
            &source,
            secret().as_ref().unwrap(),
            &ArchiveTask::new(&CancelToken::new()),
        )
        .unwrap_or_else(|_| panic!("redo damaged encryption"));
    }
}

#[test]
fn encrypted_archive_cancel_and_unsupported_formats_do_not_write_output() {
    let fixture = tempfile::tempdir().unwrap();
    let source = make(fixture.path(), ArchiveFormat::Zip);
    let output = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    let mut task = ArchiveTask::new(&cancel);
    task.set_password_prompt(|_, _| None);
    assert!(extract_archive(&LocalFs, &source, &output.path().into(), &mut task).is_err());
    assert!(cancel.is_cancelled());
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    for format in [ArchiveFormat::Tar, ArchiveFormat::TarGz] {
        let destination = VPath::from(output.path().join(format!("test.{}", format.extension())));
        let mut task = ArchiveTask::new(&CancelToken::new());
        task.set_password(secret());
        assert!(
            create_archive(
                &LocalFs,
                std::slice::from_ref(&source),
                &destination,
                format,
                &mut task
            )
            .unwrap_err()
            .contains("ZIP and 7Z")
        );
        assert!(!destination.as_path().exists());
    }
    assert!(Password::new(String::new()).is_none());
    assert_eq!(format!("{:?}", secret().unwrap()), "Password([redacted])");
}

#[test]
fn encrypted_zip_checks_private_entries_before_extracting_public_entries() {
    use zip::unstable::write::FileOptionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("mixed.zip");
    let mut writer = ZipWriter::new(fs::File::create(&source).unwrap());
    writer
        .start_file("public.txt", FileOptions::default())
        .unwrap();
    writer.write_all(b"public").unwrap();
    writer
        .start_file(
            "private.txt",
            FileOptions::default()
                .with_deprecated_encryption(SECRET.as_bytes())
                .unwrap(),
        )
        .unwrap();
    writer.write_all(b"private").unwrap();
    writer.finish().unwrap();
    let source = VPath::from(source);
    let destination = tempfile::tempdir().unwrap();
    let mut task = ArchiveTask::new(&CancelToken::new());
    task.set_password(Password::new("wrong".into()));
    assert!(extract_archive(&LocalFs, &source, &destination.path().into(), &mut task).is_err());
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
    task.set_password(secret());
    extract_archive(&LocalFs, &source, &destination.path().into(), &mut task).unwrap();
    assert_eq!(
        fs::read_to_string(destination.path().join("private.txt")).unwrap(),
        "private"
    );
}

#[test]
#[ignore = "requires the independent 7z command-line reader/writer"]
fn encrypted_archives_interoperate_with_7zip() {
    use std::process::Command;
    for format in [ArchiveFormat::Zip, ArchiveFormat::SevenZ] {
        let fixture = tempfile::tempdir().unwrap();
        let source = make(fixture.path(), format);
        let check = Command::new("7z")
            .args(["t", &format!("-p{SECRET}")])
            .arg(source.as_path())
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "{}",
            String::from_utf8_lossy(&check.stdout)
        );
        let snapshot = edit::inspect_with_password(&source, &CancelToken::new(), secret()).unwrap();
        let mut changes = edit::Changes::default();
        changes
            .renamed
            .insert("private/notes.txt".into(), "private/renamed.txt".into());
        edit::save_with_task(
            &snapshot,
            &changes,
            &mut ArchiveTask::new(&CancelToken::new()),
        )
        .unwrap();
        let check = Command::new("7z")
            .args(["t", &format!("-p{SECRET}")])
            .arg(source.as_path())
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "{}",
            String::from_utf8_lossy(&check.stdout)
        );
    }
    const EXTERNAL_SECRET: &str = "synthetic 7zip password";
    for (suffix, options) in [
        ("aes.zip", vec!["-tzip", "-mem=AES256"]),
        ("legacy.zip", vec!["-tzip", "-mem=ZipCrypto"]),
        ("hidden.7z", vec!["-t7z", "-mhe=on"]),
        ("visible.7z", vec!["-t7z", "-mhe=off"]),
    ] {
        let fixture = tempfile::tempdir().unwrap();
        fs::write(fixture.path().join("notes.txt"), "independent archive").unwrap();
        let source = fixture.path().join(suffix);
        let output = Command::new("7z")
            .current_dir(fixture.path())
            .arg("a")
            .args(options)
            .arg(format!("-p{EXTERNAL_SECRET}"))
            .arg(&source)
            .arg("notes.txt")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let destination = tempfile::tempdir().unwrap();
        let mut task = ArchiveTask::new(&CancelToken::new());
        task.set_password(Password::new("wrong".into()));
        task.set_password_prompt(|_, incorrect| {
            assert!(incorrect);
            Password::new(EXTERNAL_SECRET.into())
        });
        extract_archive(
            &LocalFs,
            &source.into(),
            &destination.path().into(),
            &mut task,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(destination.path().join("notes.txt")).unwrap(),
            "independent archive"
        );
    }
}
