//! Translate file-panel operations into one archive transaction.
use super::*;
use dualpane_engine::{Conflict, ConflictPolicy};

#[derive(Clone, Debug)]
pub enum Action {
    Copy {
        sources: Vec<VPath>,
        directory: String,
    },
    Rename {
        source: String,
        destination: String,
    },
    Remove {
        paths: Vec<String>,
    },
    Create {
        path: String,
        directory: bool,
    },
}

pub fn apply(
    snapshot: &Snapshot,
    root: &Path,
    action: &Action,
    task: &mut ArchiveTask<'_>,
    mut conflict: impl FnMut(Conflict) -> Result<ConflictPolicy, String>,
) -> Result<Option<ArchiveChange>, String> {
    // Bind the operation to the archive the user actually browsed.
    if crate::features::sha256(&LocalFs, &snapshot.source, &task.cancel)? != snapshot.digest {
        return Err(
            "Archive changed since opening. Refresh it before changing its contents.".into(),
        );
    }
    let staging = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut plan = Plan {
        root,
        staging: staging.path(),
        changes: Changes::default(),
        entries: BTreeMap::new(),
        copied: 0,
    };
    for item in &snapshot.items {
        plan.entries.insert(item.name.clone(), item.directory);
        for parent in Path::new(&item.name)
            .ancestors()
            .skip(1)
            .filter(|p| !p.as_os_str().is_empty())
        {
            plan.entries
                .entry(checked_name(
                    parent.to_str().ok_or("Invalid archive path")?,
                )?)
                .or_insert(true);
        }
    }
    match action {
        Action::Copy { sources, directory } => {
            if !directory.is_empty() {
                checked_name(directory)?;
                if plan.entries.get(directory) != Some(&true) {
                    return Err(
                        "The destination folder is no longer available in the archive.".into(),
                    );
                }
            }
            for source in sources {
                let leaf = source
                    .file_name()
                    .and_then(OsStr::to_str)
                    .ok_or("Archive entries require a UTF-8 file name")?;
                let name = if directory.is_empty() {
                    leaf.to_owned()
                } else {
                    format!("{directory}/{leaf}")
                };
                let destination = root.join(&name);
                if destination == source.as_path() || destination.starts_with(source.as_path()) {
                    return Err("Cannot copy an archive entry into itself.".into());
                }
                plan.copy(source, &name, 0, task, &mut conflict)?;
            }
        }
        Action::Rename {
            source,
            destination,
        } => {
            checked_name(source)?;
            checked_name(destination)?;
            if source == destination {
                return Ok(None);
            }
            if !plan.entries.contains_key(source) {
                return Err("The selected archive entry is no longer available.".into());
            }
            if destination.starts_with(&format!("{source}/")) {
                return Err("Cannot move an archive folder into itself.".into());
            }
            if plan.entries.contains_key(destination) {
                return Err(format!(
                    "An archive entry named “{destination}” already exists."
                ));
            }
            plan.changes
                .renamed
                .insert(source.clone(), destination.clone());
        }
        Action::Remove { paths } => {
            for path in paths {
                checked_name(path)?;
                if !plan.entries.contains_key(path) {
                    return Err("The selected archive entry is no longer available.".into());
                }
                plan.changes.removed.insert(path.clone());
            }
        }
        Action::Create { path, directory } => {
            checked_name(path)?;
            if plan.entries.contains_key(path) {
                return Err(format!("An archive entry named “{path}” already exists."));
            }
            if *directory {
                plan.changes.directories.insert(path.clone());
            } else {
                let file = staging.path().join("empty-file");
                File::create(&file).map_err(|e| e.to_string())?;
                plan.changes.added.insert(path.clone(), file);
            }
        }
    }
    if plan.changes.is_empty() {
        return Ok(None);
    }
    save_with_task(snapshot, &plan.changes, task).map(Some)
}

struct Plan<'a> {
    root: &'a Path,
    staging: &'a Path,
    changes: Changes,
    entries: BTreeMap<String, bool>,
    copied: usize,
}

impl Plan<'_> {
    fn copy(
        &mut self,
        source: &VPath,
        name: &str,
        depth: usize,
        task: &mut ArchiveTask<'_>,
        conflict: &mut impl FnMut(Conflict) -> Result<ConflictPolicy, String>,
    ) -> Result<(), String> {
        task.check()
            .map_err(|_| "Archive update cancelled".to_owned())?;
        self.copied += 1;
        if depth > 128 || self.copied > 100_000 {
            return Err("Archive update exceeds the folder depth or 100,000-entry limit.".into());
        }
        let mut name = checked_name(name)?;
        let metadata = LocalFs.stat(source, false).map_err(|e| e.to_string())?;
        if !matches!(metadata.kind, EntryKind::File | EntryKind::Directory) {
            return Err("Only regular files and folders can be copied into an archive. Symbolic links and special files are not supported.".into());
        }
        let directory = metadata.kind == EntryKind::Directory;
        let existed = self.entries.get(&name).copied();
        if let Some(existing_directory) = existed
            && !(directory && existing_directory)
        {
            let staged = self.staging.join(&name);
            let existing = if staged.exists() {
                staged
            } else {
                self.root.join(&name)
            };
            let existing = VPath::from(existing);
            let previous = LocalFs.stat(&existing, false).map_err(|e| e.to_string())?;
            match conflict(Conflict {
                source: source.clone(),
                destination: existing,
                source_metadata: metadata.clone(),
                destination_metadata: previous.clone(),
            })? {
                ConflictPolicy::Skip => return Ok(()),
                ConflictPolicy::OverwriteIfNewer if metadata.modified <= previous.modified => {
                    return Ok(());
                }
                ConflictPolicy::Rename => {
                    let renamed = dualpane_engine::unique_renamed_path(
                        &VPath::from(self.root.join(&name)),
                        |candidate| {
                            candidate
                                .as_path()
                                .strip_prefix(self.root)
                                .ok()
                                .and_then(|p| p.to_str())
                                .is_some_and(|p| self.entries.contains_key(p))
                        },
                    );
                    name = checked_name(
                        renamed
                            .as_path()
                            .strip_prefix(self.root)
                            .map_err(|e| e.to_string())?
                            .to_str()
                            .ok_or("Invalid archive path")?,
                    )?;
                }
                ConflictPolicy::Overwrite | ConflictPolicy::OverwriteIfNewer => {
                    let inside =
                        |path: &String| path == &name || path.starts_with(&format!("{name}/"));
                    self.entries.retain(|path, _| !inside(path));
                    self.changes.added.retain(|path, _| !inside(path));
                    self.changes.directories.retain(|path| !inside(path));
                    self.changes.removed.insert(name.clone());
                    let staged = self.staging.join(&name);
                    if staged.is_dir() {
                        fs::remove_dir_all(&staged).map_err(|e| e.to_string())?;
                    } else if staged.exists() {
                        fs::remove_file(&staged).map_err(|e| e.to_string())?;
                    }
                }
                _ => return Err("Unsupported archive conflict decision".into()),
            }
        }
        let staged = self.staging.join(&name);
        if directory {
            fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
            if self.entries.insert(name.clone(), true).is_none() {
                self.changes.directories.insert(name.clone());
            }
            let entries = LocalFs
                .read_dir(source, &task.cancel)
                .map_err(|e| e.to_string())?;
            for entry in entries {
                let entry = entry.map_err(|e| e.to_string())?;
                let leaf = entry
                    .name()
                    .to_str()
                    .ok_or("Archive entries require UTF-8 file names")?;
                self.copy(
                    &source.join_name(entry.name()),
                    &format!("{name}/{leaf}"),
                    depth + 1,
                    task,
                    conflict,
                )?;
            }
        } else {
            fs::create_dir_all(staged.parent().ok_or("Invalid archive destination")?)
                .map_err(|e| e.to_string())?;
            task.begin(source);
            let mut output = File::create(&staged).map_err(|e| e.to_string())?;
            let copied = copy_reader(
                LocalFs.open_read(source).map_err(|e| e.to_string())?,
                &mut output,
                task,
            )?;
            let after = LocalFs.stat(source, false).map_err(|e| e.to_string())?;
            if copied != metadata.size
                || after.size != metadata.size
                || after.identity != metadata.identity
                || after.modified != metadata.modified
                || after.kind != metadata.kind
            {
                return Err(format!(
                    "{source} changed while reading it. The archive was not changed."
                ));
            }
            output
                .set_permissions(
                    fs::metadata(source.as_path())
                        .map_err(|e| e.to_string())?
                        .permissions(),
                )
                .map_err(|e| e.to_string())?;
            self.changes.added.insert(name.clone(), staged);
            self.entries.insert(name, false);
            task.item_done();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contents(source: &VPath) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        extract_archive(
            &LocalFs,
            source,
            &VPath::from(directory.path()),
            &mut ArchiveTask::new(&CancelToken::new()),
        )
        .unwrap();
        directory
    }

    fn update(source: &VPath, action: Action, policy: ConflictPolicy) -> Option<ArchiveChange> {
        let token = CancelToken::new();
        let snapshot = inspect(source, &token).unwrap();
        let previous = fs::read(source.as_path()).unwrap();
        let root = contents(source);
        let backup = apply(
            &snapshot,
            root.path(),
            &action,
            &mut ArchiveTask::new(&token),
            |_| Ok(policy),
        )
        .unwrap();
        if let Some(path) = &backup {
            assert_eq!(fs::read(path.backup.as_path()).unwrap(), previous);
        }
        backup
    }

    #[test]
    fn panel_operations_preserve_archives_add_folders_rename_and_remove_in_all_formats() {
        let fixture = tempfile::tempdir().unwrap();
        let original = fixture.path().join("original.txt");
        fs::write(&original, "original").unwrap();
        let incoming = fixture.path().join("Incoming");
        fs::create_dir_all(incoming.join("Empty")).unwrap();
        fs::write(incoming.join("note.txt"), "new contents").unwrap();
        for format in [
            ArchiveFormat::Zip,
            ArchiveFormat::SevenZ,
            ArchiveFormat::Tar,
            ArchiveFormat::TarGz,
        ] {
            let source = VPath::from(
                fixture
                    .path()
                    .join(format!("sample.{}", format.extension())),
            );
            create_archive(
                &LocalFs,
                &[VPath::from(original.as_path())],
                &source,
                format,
                &mut ArchiveTask::new(&CancelToken::new()),
            )
            .unwrap();
            update(
                &source,
                Action::Copy {
                    sources: vec![VPath::from(incoming.as_path())],
                    directory: String::new(),
                },
                ConflictPolicy::Overwrite,
            );
            update(
                &source,
                Action::Create {
                    path: "Incoming/New folder".into(),
                    directory: true,
                },
                ConflictPolicy::Overwrite,
            );
            update(
                &source,
                Action::Create {
                    path: "Incoming/empty.txt".into(),
                    directory: false,
                },
                ConflictPolicy::Overwrite,
            );
            update(
                &source,
                Action::Rename {
                    source: "Incoming".into(),
                    destination: "Renamed".into(),
                },
                ConflictPolicy::Overwrite,
            );
            update(
                &source,
                Action::Rename {
                    source: "Renamed/note.txt".into(),
                    destination: "Renamed/renamed.txt".into(),
                },
                ConflictPolicy::Overwrite,
            );
            let unpacked = contents(&source);
            assert_eq!(
                fs::read(unpacked.path().join("original.txt")).unwrap(),
                b"original"
            );
            assert_eq!(
                fs::read(unpacked.path().join("Renamed/renamed.txt")).unwrap(),
                b"new contents"
            );
            assert!(unpacked.path().join("Renamed/Empty").is_dir());
            assert!(unpacked.path().join("Renamed/New folder").is_dir());
            assert_eq!(
                fs::metadata(unpacked.path().join("Renamed/empty.txt"))
                    .unwrap()
                    .len(),
                0
            );
            assert!(!unpacked.path().join("Incoming").exists());
            update(
                &source,
                Action::Remove {
                    paths: vec!["Renamed".into()],
                },
                ConflictPolicy::Overwrite,
            );
            let unpacked = contents(&source);
            assert!(!unpacked.path().join("Renamed").exists());
            assert_eq!(
                fs::read(unpacked.path().join("original.txt")).unwrap(),
                b"original"
            );
        }
    }

    #[test]
    fn conflicts_skip_keep_both_replace_and_cancel_without_changing_the_source() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("note.txt");
        fs::write(&file, "old").unwrap();
        let source = VPath::from(fixture.path().join("sample.zip"));
        create_archive(
            &LocalFs,
            &[VPath::from(file.as_path())],
            &source,
            ArchiveFormat::Zip,
            &mut ArchiveTask::new(&CancelToken::new()),
        )
        .unwrap();
        fs::write(&file, "new").unwrap();
        let action = Action::Copy {
            sources: vec![VPath::from(file.as_path())],
            directory: String::new(),
        };
        assert!(update(&source, action.clone(), ConflictPolicy::Skip).is_none());
        update(&source, action.clone(), ConflictPolicy::Rename);
        let root = contents(&source);
        assert_eq!(fs::read(root.path().join("note.txt")).unwrap(), b"old");
        assert_eq!(fs::read(root.path().join("note (2).txt")).unwrap(), b"new");
        update(&source, action.clone(), ConflictPolicy::Overwrite);
        assert_eq!(
            fs::read(contents(&source).path().join("note.txt")).unwrap(),
            b"new"
        );
        let snapshot = inspect(&source, &CancelToken::new()).unwrap();
        let before = fs::read(source.as_path()).unwrap();
        let token = CancelToken::new();
        let result = apply(
            &snapshot,
            root.path(),
            &action,
            &mut ArchiveTask::new(&token),
            |_| {
                token.cancel();
                Ok(ConflictPolicy::Overwrite)
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read(source.as_path()).unwrap(), before);
        let stale = Snapshot {
            digest: "invalid".into(),
            ..snapshot
        };
        assert!(
            apply(
                &stale,
                root.path(),
                &Action::Remove {
                    paths: vec!["note.txt".into()]
                },
                &mut ArchiveTask::new(&CancelToken::new()),
                |_| unreachable!()
            )
            .is_err()
        );
        assert_eq!(fs::read(source.as_path()).unwrap(), before);
    }

    #[test]
    fn rejected_paths_and_unsupported_sources_leave_the_archive_unchanged() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("note.txt");
        fs::write(&file, "original").unwrap();
        let source = VPath::from(fixture.path().join("sample.zip"));
        create_archive(
            &LocalFs,
            &[VPath::from(file.as_path())],
            &source,
            ArchiveFormat::Zip,
            &mut ArchiveTask::new(&CancelToken::new()),
        )
        .unwrap();
        let root = contents(&source);
        let snapshot = inspect(&source, &CancelToken::new()).unwrap();
        let before = fs::read(source.as_path()).unwrap();
        let link = fixture.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        for action in [
            Action::Create {
                path: "../outside".into(),
                directory: true,
            },
            Action::Create {
                path: "note.txt/child".into(),
                directory: false,
            },
            Action::Rename {
                source: "note.txt".into(),
                destination: "note.txt/child".into(),
            },
            Action::Remove {
                paths: vec![String::new()],
            },
            Action::Copy {
                sources: vec![VPath::from(link.as_path())],
                directory: String::new(),
            },
        ] {
            assert!(
                apply(
                    &snapshot,
                    root.path(),
                    &action,
                    &mut ArchiveTask::new(&CancelToken::new()),
                    |_| unreachable!()
                )
                .is_err()
            );
            assert_eq!(fs::read(source.as_path()).unwrap(), before);
        }
    }
    #[test]
    fn cancelling_during_copy_and_changed_permissions_preserve_original_bytes() {
        use dualpane_engine::JobControl;
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let input = fixture.path().join("incoming.bin");
        fs::write(&input, vec![42u8; 1024 * 1024]).unwrap();
        let source = VPath::from(fixture.path().join("test.zip"));
        let cancel = CancelToken::new();
        create_archive(
            &LocalFs,
            &[VPath::from(input.as_path())],
            &source,
            ArchiveFormat::Zip,
            &mut ArchiveTask::new(&cancel),
        )
        .unwrap();
        let original = fs::read(source.as_path()).unwrap();
        let snapshot = inspect(&source, &cancel).unwrap();
        let root = fixture.path().join("extracted");
        fs::create_dir(&root).unwrap();
        extract_archive(
            &LocalFs,
            &source,
            &VPath::from(root.as_path()),
            &mut ArchiveTask::new(&cancel),
        )
        .unwrap();
        let control = JobControl::new();
        let stop = control.clone();
        let action = Action::Copy {
            sources: vec![VPath::from(input.as_path())],
            directory: String::new(),
        };
        let result = apply(
            &snapshot,
            &root,
            &action,
            &mut ArchiveTask::with_progress(&control, move |progress| {
                if progress.bytes_done > 0 {
                    stop.cancel();
                }
            }),
            |_| Ok(ConflictPolicy::Overwrite),
        );
        assert!(result.is_err());
        assert!(control.cancel_token().is_cancelled());
        assert_eq!(fs::read(source.as_path()).unwrap(), original);
        fs::set_permissions(source.as_path(), fs::Permissions::from_mode(0o444)).unwrap();
        let result = apply(
            &snapshot,
            &root,
            &Action::Create {
                path: "folder".into(),
                directory: true,
            },
            &mut ArchiveTask::new(&cancel),
            |_| unreachable!(),
        );
        assert!(result.unwrap_err().contains("read-only"));
        assert_eq!(fs::read(source.as_path()).unwrap(), original);
    }
}
