use super::*;
use std::cell::Cell;
use std::fs;
use std::io::Read;
use std::os::unix::fs::symlink;

fn plan(paths: &[PathBuf]) -> SecureDeletePlan {
    review_secure_delete(paths, &mut || Ok(())).unwrap()
}

#[test]
fn overwrites_verifies_and_removes_only_reviewed_files() {
    let root = tempfile::tempdir().unwrap();
    let one = root.path().join("private.txt");
    let empty = root.path().join("empty");
    let keep = root.path().join("keep");
    fs::write(&one, vec![123; 600_001]).unwrap();
    fs::write(&empty, []).unwrap();
    fs::write(&keep, "unchanged").unwrap();
    let plan = plan(&[one.clone(), empty.clone(), one.clone()]);
    let mut original_inode = fs::File::open(&one).unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan.bytes, 600_001);
    let mut phases = Vec::new();
    assert_eq!(
        secure_delete(&plan, &mut || Ok(()), &mut |p| phases.push(p)).unwrap(),
        2
    );
    assert!(!one.exists() && !empty.exists());
    let mut erased = Vec::new();
    original_inode.read_to_end(&mut erased).unwrap();
    assert_eq!(erased, vec![0; 600_001]);
    assert_eq!(fs::read_to_string(keep).unwrap(), "unchanged");
    assert!(phases.iter().any(|p| p.verifying));
    assert_eq!(phases.last().unwrap().bytes_done, 1_200_002);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn rejects_links_directories_devices_and_symlink_ancestors() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    let link = root.path().join("link");
    fs::write(&file, "private").unwrap();
    symlink(&file, &link).unwrap();
    for path in [
        &link,
        &root.path().to_path_buf(),
        &PathBuf::from("/dev/null"),
    ] {
        assert!(review_secure_delete(std::slice::from_ref(path), &mut || Ok(())).is_err());
    }
    fs::remove_file(&link).unwrap();
    fs::hard_link(&file, &link).unwrap();
    assert!(review_secure_delete(std::slice::from_ref(&file), &mut || Ok(())).is_err());
    fs::remove_file(&link).unwrap();
    symlink(root.path(), &link).unwrap();
    assert!(review_secure_delete(&[link.join("file")], &mut || Ok(())).is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "private");
}

#[test]
fn stale_batch_is_rejected_before_any_file_is_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    fs::write(&first, "first secret").unwrap();
    fs::write(&second, "second secret").unwrap();
    let plan = plan(&[first.clone(), second.clone()]);
    fs::remove_file(&second).unwrap();
    fs::write(&second, "replacement").unwrap();
    assert!(secure_delete(&plan, &mut || Ok(()), &mut |_| {}).is_err());
    assert_eq!(fs::read_to_string(first).unwrap(), "first secret");
    assert_eq!(fs::read_to_string(second).unwrap(), "replacement");
}

#[test]
fn cancel_before_writing_keeps_original_and_midway_retains_partial_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("secret");
    let original = vec![55; 700_000];
    fs::write(&path, &original).unwrap();
    let plan = plan(std::slice::from_ref(&path));
    let cancel = || Err(io::Error::new(io::ErrorKind::Interrupted, "Cancelled"));
    assert!(secure_delete(&plan, &mut || cancel(), &mut |_| {}).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    let stop = Cell::new(false);
    assert!(
        secure_delete(
            &plan,
            &mut || if stop.get() { cancel() } else { Ok(()) },
            &mut |_| stop.set(true)
        )
        .is_err()
    );
    let retained = fs::read(&path).unwrap();
    assert_eq!(retained.len(), original.len());
    assert!(retained[..256 * 1024].iter().all(|byte| *byte == 0));
    assert_eq!(&retained[256 * 1024..], &original[256 * 1024..]);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn replacement_during_operation_is_never_overwritten_or_unlinked() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("secret");
    fs::write(&path, vec![77; 500_000]).unwrap();
    let plan = plan(std::slice::from_ref(&path));
    let replaced = Cell::new(false);
    secure_delete(&plan, &mut || Ok(()), &mut |_| {
        if !replaced.replace(true) {
            fs::write(&path, "new unrelated file").unwrap();
        }
    })
    .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new unrelated file");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn cancelled_file_is_preserved_with_recovery_path_if_original_name_is_reused() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("secret");
    fs::write(&path, vec![77; 500_000]).unwrap();
    let plan = plan(std::slice::from_ref(&path));
    let stop = Cell::new(false);
    let error = secure_delete(
        &plan,
        &mut || {
            if stop.get() {
                Err(io::Error::other("Cancelled"))
            } else {
                Ok(())
            }
        },
        &mut |_| {
            fs::write(&path, "replacement").unwrap();
            stop.set(true);
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("Retained file:"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
    let private = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    assert_eq!(
        fs::metadata(private.join("contents")).unwrap().len(),
        500_000
    );
}

#[test]
fn failed_readback_keeps_file_and_reports_verification_failure() {
    use std::io::{Seek, SeekFrom, Write};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("secret");
    fs::write(&path, vec![77; 600_000]).unwrap();
    let plan = plan(std::slice::from_ref(&path));
    let changed = Cell::new(false);
    let error = secure_delete(&plan, &mut || Ok(()), &mut |progress| {
        if progress.verifying && !changed.replace(true) {
            let private = fs::read_dir(root.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.is_dir())
                .unwrap();
            let mut file = fs::OpenOptions::new()
                .write(true)
                .open(private.join("contents"))
                .unwrap();
            file.seek(SeekFrom::Start(500_000)).unwrap();
            file.write_all(b"changed").unwrap();
        }
    })
    .unwrap_err();
    assert!(error.to_string().contains("verification failed"));
    assert!(path.exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}
