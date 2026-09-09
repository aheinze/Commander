//! Archive transactions stream unchanged entries and publish only complete output.
use super::*;
use dualpane_vfs::LocalFs;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
};

mod operation;
mod transaction;
use dualpane_engine::journal::ArchiveChange;
pub use operation::{Action, apply};
pub use transaction::restore;

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub directory: bool,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub source: VPath,
    pub format: ArchiveFormat,
    pub digest: String,
    pub items: Vec<Item>,
    pub(super) password: Option<Password>,
}
#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub removed: BTreeSet<String>,
    pub added: BTreeMap<String, PathBuf>,
    pub directories: BTreeSet<String>,
    pub renamed: BTreeMap<String, String>,
}
impl Changes {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty()
            && self.added.is_empty()
            && self.directories.is_empty()
            && self.renamed.is_empty()
    }

    fn renamed_path(&self, name: &str) -> String {
        self.renamed
            .iter()
            .filter_map(|(source, destination)| {
                if name == source {
                    Some((source.len(), destination.clone()))
                } else {
                    name.strip_prefix(&format!("{source}/"))
                        .map(|relative| (source.len(), format!("{destination}/{relative}")))
                }
            })
            .max_by_key(|(length, _)| *length)
            .map_or_else(|| name.to_owned(), |(_, path)| path)
    }

    fn excludes(&self, name: &str) -> bool {
        self.added.contains_key(name)
            || self.removed.iter().any(|removed| {
                name == removed || name.starts_with(&format!("{}/", removed.trim_end_matches('/')))
            })
    }
}
fn checked_name(name: &str) -> Result<String, String> {
    if name.contains(['\\', '\0']) || !safe_relative_path(Path::new(name)) {
        return Err(format!("Unsafe archive path: {name:?}"));
    }
    let normalized = Path::new(name)
        .components()
        .filter_map(|part| match part {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        return Err(format!("Empty archive path: {name:?}"));
    }
    Ok(normalized)
}
fn root_directory(path: &Path, header: &TarHeader) -> bool {
    header.entry_type().is_dir() && path.components().all(|part| part == Component::CurDir)
}
fn format_for(source: &VPath) -> Result<ArchiveFormat, String> {
    let name = source.to_string().to_lowercase();
    if name.ends_with(".zip") {
        Ok(ArchiveFormat::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Ok(ArchiveFormat::TarGz)
    } else if name.ends_with(".tar") {
        Ok(ArchiveFormat::Tar)
    } else if name.ends_with(".7z") {
        Ok(ArchiveFormat::SevenZ)
    } else {
        Err("Editable formats: ZIP, 7Z, TAR, TAR.GZ and TGZ".into())
    }
}
#[cfg(test)]
pub fn inspect(source: &VPath, cancel: &CancelToken) -> Result<Snapshot, String> {
    inspect_with_password(source, cancel, None)
}

pub fn inspect_with_password(
    source: &VPath,
    cancel: &CancelToken,
    password: Option<Password>,
) -> Result<Snapshot, String> {
    let format = format_for(source)?;
    let metadata = fs::symlink_metadata(source.as_path()).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Choose a regular archive file; symbolic links cannot be edited.".into());
    }
    let digest = crate::features::sha256(&LocalFs, source, cancel)?;
    let mut items = Vec::new();
    let file = File::open(source.as_path()).map_err(|e| e.to_string())?;
    match format {
        ArchiveFormat::Zip => {
            let mut archive = ZipArchive::new(Cancellable {
                inner: file,
                cancel,
                control: None,
            })
            .map_err(|e| e.to_string())?;
            if archive.len() > 100_000 {
                return Err("Archive exceeds 100,000 entries".into());
            }
            for index in 0..archive.len() {
                cancel
                    .check()
                    .map_err(|_| "Archive scan cancelled".to_owned())?;
                let entry = archive.by_index_raw(index).map_err(|e| e.to_string())?;
                if entry.encrypted() && password.is_none() {
                    return Err("Unlock this archive before editing it".into());
                }
                items.push(Item {
                    name: checked_name(entry.name())?,
                    directory: entry.is_dir(),
                });
            }
        }
        ArchiveFormat::Tar | ArchiveFormat::TarGz => {
            let reader: Box<dyn Read> = if format == ArchiveFormat::TarGz {
                Box::new(GzDecoder::new(file))
            } else {
                Box::new(file)
            };
            let reader = Cancellable {
                inner: reader,
                cancel,
                control: None,
            };
            for entry in TarArchive::new(reader)
                .entries()
                .map_err(|e| e.to_string())?
            {
                if items.len() >= 100_000 {
                    return Err("Archive exceeds 100,000 entries".into());
                }
                let mut entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path().map_err(|e| e.to_string())?.into_owned();
                // Rewriting sparse or extended metadata needs format-specific preservation.
                if !matches!(
                    entry.header().entry_type(),
                    tar::EntryType::Regular
                        | tar::EntryType::Directory
                        | tar::EntryType::Symlink
                        | tar::EntryType::Link
                ) || entry.pax_extensions().map_err(|e| e.to_string())?.is_some()
                {
                    return Err("This TAR contains extended or sparse entries that cannot yet be edited without losing metadata.".into());
                }
                if root_directory(&path, entry.header()) {
                    continue;
                }
                items.push(Item {
                    name: checked_name(path.to_str().ok_or("Archive names must be valid UTF-8")?)?,
                    directory: entry.header().entry_type().is_dir(),
                });
            }
        }
        ArchiveFormat::SevenZ => {
            let archive = SevenZReader::new(
                Cancellable {
                    inner: file,
                    cancel,
                    control: None,
                },
                password::sevenz_password(password.as_ref()),
            )
            .map_err(|e| e.to_string())?;
            if password.is_none()
                && archive.archive().blocks.iter().any(|block| {
                    block.coders.iter().any(|coder| {
                        coder.encoder_method_id() == sevenz_rust2::EncoderMethod::ID_AES256_SHA256
                    })
                })
            {
                return Err("Unlock this archive before editing it".into());
            }
            for entry in &archive.archive().files {
                cancel
                    .check()
                    .map_err(|_| "Archive scan cancelled".to_owned())?;
                items.push(Item {
                    name: checked_name(&entry.name)?,
                    directory: entry.is_directory,
                });
            }
        }
    }
    if items.len() > 100_000 {
        return Err("Archive has more than 100,000 entries. Use an external archive tool.".into());
    }
    let mut names = BTreeSet::new();
    for item in &items {
        if !names.insert(&item.name) {
            return Err(format!("Archive has duplicate paths: {}", item.name));
        }
    }
    if crate::features::sha256(&LocalFs, source, cancel)? != digest {
        return Err("Archive changed while it was being opened. Open it again.".into());
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Snapshot {
        source: source.clone(),
        format,
        digest,
        items,
        password,
    })
}

#[cfg(test)]
pub fn save(
    snapshot: &Snapshot,
    changes: &Changes,
    cancel: &CancelToken,
) -> Result<PathBuf, String> {
    save_with_task(snapshot, changes, &mut ArchiveTask::new(cancel))
        .map(|change| change.backup.as_path().to_owned())
}

fn writable_permissions(source: &VPath) -> Result<fs::Permissions, String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(source.as_path()).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err(
            "The archive is no longer a regular file. Refresh it before making changes.".into(),
        );
    }
    if metadata.permissions().mode() & 0o222 == 0 {
        return Err("The archive file is read-only.".into());
    }
    Ok(metadata.permissions())
}

pub fn save_with_task(
    snapshot: &Snapshot,
    changes: &Changes,
    task: &mut ArchiveTask<'_>,
) -> Result<ArchiveChange, String> {
    let cancel = task.cancel.clone();
    let cancel = &cancel;
    writable_permissions(&snapshot.source)?;
    if changes.is_empty() {
        return Err("No archive changes to save".into());
    }
    for name in changes
        .removed
        .iter()
        .chain(changes.added.keys())
        .chain(changes.directories.iter())
        .chain(changes.renamed.keys())
        .chain(changes.renamed.values())
    {
        if checked_name(name)? != *name {
            return Err("Use normalized archive paths without './' or repeated separators".into());
        }
    }
    let mut final_items = BTreeMap::new();
    for item in snapshot
        .items
        .iter()
        .filter(|item| !changes.excludes(&item.name))
    {
        let name = changes.renamed_path(&item.name);
        if final_items.insert(name.clone(), item.directory).is_some() {
            return Err(format!("Archive path already exists: {name}"));
        }
    }
    for name in &changes.directories {
        if final_items.insert(name.clone(), true).is_some() {
            return Err(format!("Archive path already exists: {name}"));
        }
    }
    for (name, source) in &changes.added {
        let metadata =
            fs::symlink_metadata(source).map_err(|e| format!("{}: {e}", source.display()))?;
        if !metadata.is_file() {
            return Err("The staged archive input is not a regular file.".into());
        }
        final_items.insert(name.clone(), false);
    }
    if final_items.len() > 100_000 {
        return Err("The updated archive would exceed 100,000 entries.".into());
    }
    for name in final_items.keys() {
        let mut parent = Path::new(name).parent();
        while let Some(path) = parent {
            if final_items.get(&path.to_string_lossy().into_owned()) == Some(&false) {
                return Err(format!("A file blocks the folder path for {name}"));
            }
            parent = path.parent();
        }
    }
    if crate::features::sha256(&LocalFs, &snapshot.source, cancel)? != snapshot.digest {
        return Err(
            "Archive changed since opening. Refresh the archive before changing its contents."
                .into(),
        );
    }
    let parent = snapshot
        .source
        .as_path()
        .parent()
        .ok_or("Archive has no parent directory")?;
    let mut output = tempfile::Builder::new()
        .prefix(".commander-archive-edit-")
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    let file = File::open(snapshot.source.as_path()).map_err(|e| e.to_string())?;
    match snapshot.format {
        ArchiveFormat::Zip => {
            let mut input = ZipArchive::new(Cancellable {
                inner: file,
                cancel,
                control: task.control(),
            })
            .map_err(|e| e.to_string())?;
            let mut writer = ZipWriter::new(output.as_file_mut());
            writer
                .set_raw_comment(input.comment().into())
                .map_err(|e| e.to_string())?;
            for index in 0..input.len() {
                task.check()
                    .map_err(|_| "Archive save cancelled".to_owned())?;
                let mut entry = if let Some(password) = &snapshot.password {
                    input.by_index_decrypt(index, password.expose().as_bytes())
                } else {
                    input.by_index(index)
                }
                .map_err(|e| e.to_string())?;
                let name = checked_name(entry.name())?;
                if !changes.excludes(&name) {
                    let size = entry.size();
                    task.begin(&snapshot.source.as_path().join(&name).into());
                    let mut destination = changes.renamed_path(&name);
                    if entry.is_dir() {
                        destination.push('/');
                    }
                    if entry.encrypted() && !entry.is_dir() {
                        let password = snapshot
                            .password
                            .as_ref()
                            .ok_or("Unlock this archive before editing it")?;
                        let options = entry
                            .options()
                            .compression_method(CompressionMethod::Deflated)
                            .with_aes_encryption(zip::AesMode::Aes256, password.expose())
                            .into_full_options()
                            .with_file_comment(entry.comment());
                        writer
                            .start_file(destination, options)
                            .map_err(|e| e.to_string())?;
                        copy_reader(&mut entry, &mut writer, task)?;
                    } else {
                        writer
                            .raw_copy_file_rename(entry, destination)
                            .map_err(|e| e.to_string())?;
                        task.advanced(size);
                    }
                    task.item_done();
                }
            }
            for name in &changes.directories {
                task.check()
                    .map_err(|_| "Archive save cancelled".to_owned())?;
                writer
                    .add_directory(
                        format!("{name}/"),
                        FileOptions::default().unix_permissions(0o755),
                    )
                    .map_err(|e| e.to_string())?;
                task.item_done();
            }
            for (name, source) in &changes.added {
                add_zip_path(
                    &LocalFs,
                    &mut writer,
                    &VPath::from(source.as_path()),
                    PathBuf::from(name),
                    password::zip_options(snapshot.password.as_ref()),
                    task,
                )?;
            }
            writer.finish().map_err(|e| e.to_string())?;
        }
        ArchiveFormat::Tar | ArchiveFormat::TarGz => {
            let input: Box<dyn Read> = if snapshot.format == ArchiveFormat::TarGz {
                Box::new(GzDecoder::new(file))
            } else {
                Box::new(file)
            };
            if snapshot.format == ArchiveFormat::TarGz {
                let mut writer =
                    TarBuilder::new(GzEncoder::new(output.as_file_mut(), Compression::default()));
                rewrite_tar(input, &mut writer, changes, task)?;
                writer
                    .into_inner()
                    .map_err(|e| e.to_string())?
                    .finish()
                    .map_err(|e| e.to_string())?;
            } else {
                let mut writer = TarBuilder::new(output.as_file_mut());
                rewrite_tar(input, &mut writer, changes, task)?;
                writer.finish().map_err(|e| e.to_string())?;
            }
        }
        ArchiveFormat::SevenZ => {
            let mut reader = SevenZReader::new(
                Cancellable {
                    inner: file,
                    cancel,
                    control: task.control(),
                },
                password::sevenz_password(snapshot.password.as_ref()),
            )
            .map_err(|e| e.to_string())?;
            let mut writer = SevenZWriter::new(output.as_file_mut()).map_err(|e| e.to_string())?;
            password::configure_sevenz(&mut writer, snapshot.password.as_ref());
            reader
                .for_each_entries(|entry, reader| {
                    let failure = |e: String| sevenz_rust2::Error::from(io::Error::other(e));
                    task.check()
                        .map_err(|_| failure("Archive save cancelled".into()))?;
                    if !changes.excludes(&checked_name(&entry.name).map_err(failure)?) {
                        let mut preserved = entry.clone();
                        preserved.name =
                            changes.renamed_path(&checked_name(&entry.name).map_err(failure)?);
                        // sevenz-rust2 0.20.2 writes the complement of the anti-item bit
                        // for streamless entries. Compensate so the original bit survives.
                        if !preserved.has_stream {
                            preserved.is_anti_item = !preserved.is_anti_item;
                        }
                        writer.push_archive_entry(
                            preserved,
                            Some(ProgressReader {
                                inner: reader,
                                task,
                            }),
                        )?;
                        task.item_done();
                    } else {
                        io::copy(
                            &mut Cancellable {
                                inner: reader,
                                cancel,
                                control: task.control(),
                            },
                            &mut io::sink(),
                        )
                        .map_err(sevenz_rust2::Error::from)?;
                    }
                    Ok(true)
                })
                .map_err(|e| e.to_string())?;
            for name in &changes.directories {
                task.check()
                    .map_err(|_| "Archive save cancelled".to_owned())?;
                let mut entry = SevenZEntry::new_directory(name);
                entry.is_anti_item = true; // Compensate for the streamless-entry writer bit.
                writer
                    .push_archive_entry(entry, None::<io::Empty>)
                    .map_err(|e| e.to_string())?;
                task.item_done();
            }
            for (name, source) in &changes.added {
                let file = File::open(source).map_err(|e| e.to_string())?;
                writer
                    .push_archive_entry(
                        SevenZEntry::new_file(name),
                        Some(ProgressReader { inner: file, task }),
                    )
                    .map_err(|e| e.to_string())?;
                task.item_done();
            }
            writer.finish().map_err(|e| e.to_string())?;
        }
    }
    transaction::publish(snapshot, output, task)
}

fn rewrite_tar<W: Write>(
    input: Box<dyn Read>,
    writer: &mut TarBuilder<W>,
    changes: &Changes,
    task: &mut ArchiveTask<'_>,
) -> Result<(), String> {
    let cancel = task.cancel.clone();
    for entry in TarArchive::new(Cancellable {
        inner: input,
        cancel: &cancel,
        control: task.control(),
    })
    .entries()
    .map_err(|e| e.to_string())?
    {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        task.check()
            .map_err(|_| "Archive save cancelled".to_owned())?;
        let root = root_directory(&path, entry.header());
        let name = if root {
            String::new()
        } else {
            checked_name(path.to_str().ok_or("Invalid archive name")?)?
        };
        if root || !changes.excludes(&name) {
            let mut header = entry.header().clone();
            if header.entry_type().is_hard_link()
                && let Some(link) = entry.link_name().map_err(|e| e.to_string())?
            {
                let name = checked_name(link.to_str().ok_or("Invalid archive link name")?)?;
                header
                    .set_link_name(changes.renamed_path(&name))
                    .map_err(|e| e.to_string())?;
            }
            let destination = if root {
                path
            } else {
                PathBuf::from(changes.renamed_path(&name))
            };
            writer
                .append_data(
                    &mut header,
                    &destination,
                    ProgressReader {
                        inner: &mut entry,
                        task,
                    },
                )
                .map_err(|e| e.to_string())?;
            task.item_done();
        }
    }
    for name in &changes.directories {
        task.check()
            .map_err(|_| "Archive save cancelled".to_owned())?;
        let mut header = TarHeader::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_mode(0o755);
        header.set_size(0);
        header.set_cksum();
        writer
            .append_data(&mut header, name, io::empty())
            .map_err(|e| e.to_string())?;
        task.item_done();
    }
    for (name, source) in &changes.added {
        add_tar_path(
            &LocalFs,
            writer,
            &VPath::from(source.as_path()),
            PathBuf::from(name),
            task,
        )?;
    }
    Ok(())
}
struct Cancellable<'a, R> {
    inner: R,
    cancel: &'a CancelToken,
    control: Option<dualpane_engine::JobControl>,
}
impl<R: Read> Read for Cancellable<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.control
            .as_ref()
            .map_or_else(
                || self.cancel.check(),
                dualpane_engine::JobControl::checkpoint,
            )
            .map_err(|_| io::Error::other("Archive operation cancelled"))?;
        let length = buffer.len().min(BUFFER_SIZE);
        self.inner.read(&mut buffer[..length])
    }
}

impl<R: io::Seek> io::Seek for Cancellable<'_, R> {
    fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
        self.control
            .as_ref()
            .map_or_else(
                || self.cancel.check(),
                dualpane_engine::JobControl::checkpoint,
            )
            .map_err(|_| io::Error::other("Archive operation cancelled"))?;
        self.inner.seek(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_reader_respects_pause_and_cancellation_between_chunks() {
        use dualpane_engine::JobControl;
        let control = JobControl::new();
        control.pause();
        let worker_control = control.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let cancel = worker_control.cancel_token();
            let mut reader = Cancellable {
                inner: io::Cursor::new([42u8; 8]),
                cancel: &cancel,
                control: Some(worker_control),
            };
            tx.send(reader.read(&mut [0u8; 8])).unwrap();
        });
        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_millis(30)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        control.cancel();
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
        worker.join().unwrap();
    }

    #[test]
    fn writable_archives_round_trip_add_replace_remove_and_keep_backup() {
        for format in [
            ArchiveFormat::Zip,
            ArchiveFormat::Tar,
            ArchiveFormat::TarGz,
            ArchiveFormat::SevenZ,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let folder = dir.path().join("folder");
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("keep.txt"), b"original").unwrap();
            fs::write(folder.join("replace.txt"), b"old").unwrap();
            fs::write(folder.join("remove.txt"), b"remove").unwrap();
            let source = VPath::from(dir.path().join(format!("archive.{}", format.extension())));
            let token = CancelToken::new();
            create_archive(
                &LocalFs,
                &[VPath::from(&*folder)],
                &source,
                format,
                &mut ArchiveTask::new(&token),
            )
            .unwrap();
            let before = fs::read(source.as_path()).unwrap();
            let snapshot = inspect(&source, &token).unwrap();
            let extra = dir.path().join("extra.txt");
            fs::write(&extra, b"new content").unwrap();
            let mut changes = Changes::default();
            changes.removed.insert("folder/remove.txt".into());
            changes
                .added
                .insert("folder/replace.txt".into(), extra.clone());
            changes.added.insert("new/nested.txt".into(), extra);
            let backup = save(&snapshot, &changes, &token).unwrap();
            assert_eq!(fs::read(backup).unwrap(), before);
            let out = dir.path().join("out");
            fs::create_dir(&out).unwrap();
            extract_archive(
                &LocalFs,
                &source,
                &VPath::from(out.as_path()),
                &mut ArchiveTask::new(&token),
            )
            .unwrap();
            assert_eq!(fs::read(out.join("folder/keep.txt")).unwrap(), b"original");
            assert_eq!(
                fs::read(out.join("folder/replace.txt")).unwrap(),
                b"new content"
            );
            assert_eq!(
                fs::read(out.join("new/nested.txt")).unwrap(),
                b"new content"
            );
            assert!(!out.join("folder/remove.txt").exists());
            let stale = save(&snapshot, &changes, &token).unwrap_err();
            assert!(stale.contains("changed since"), "{stale}");
            let fresh = inspect(&source, &token).unwrap();
            let saved = fs::read(source.as_path()).unwrap();
            let cancelled = CancelToken::new();
            cancelled.cancel();
            assert!(save(&fresh, &changes, &cancelled).is_err());
            assert_eq!(fs::read(source.as_path()).unwrap(), saved);
            let mut unsafe_change = Changes::default();
            unsafe_change.removed.insert("../outside".into());
            assert!(save(&fresh, &unsafe_change, &token).is_err());
        }
    }
    #[test]
    fn writable_archives_handle_tar_dot_roots_and_normalize_entry_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = VPath::from(dir.path().join("dot.tar"));
        let mut archive = TarBuilder::new(File::create(path.as_path()).unwrap());
        let mut header = TarHeader::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        archive.append_data(&mut header, ".", io::empty()).unwrap();
        let mut header = TarHeader::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(3);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "./old.txt", &b"old"[..])
            .unwrap();
        archive.finish().unwrap();
        let cancel = CancelToken::new();
        let snapshot = inspect(&path, &cancel).unwrap();
        assert_eq!(snapshot.items[0].name, "old.txt");
        let extra = dir.path().join("extra");
        fs::write(&extra, "new").unwrap();
        let mut changes = Changes::default();
        changes.added.insert("old.txt".into(), extra);
        save(&snapshot, &changes, &cancel).unwrap();
        let updated = inspect(&path, &cancel).unwrap();
        assert_eq!(updated.items.len(), 1);
        assert_eq!(checked_name("./a//b/").unwrap(), "a/b");
        assert!(checked_name("a/../b").is_err());
    }

    #[test]
    fn writable_archives_preserve_zip_metadata_and_tar_links() {
        let dir = tempfile::tempdir().unwrap();
        let source = VPath::from(dir.path().join("archive.zip"));
        let mut zip = ZipWriter::new(File::create(source.as_path()).unwrap());
        zip.set_comment("Keep this comment").unwrap();
        zip.start_file("script.sh", FileOptions::default().unix_permissions(0o755))
            .unwrap();
        zip.write_all(b"#!/bin/sh\n").unwrap();
        zip.finish().unwrap();
        let token = CancelToken::new();
        let mut changes = Changes::default();
        let extra = dir.path().join("extra");
        fs::write(&extra, b"extra").unwrap();
        changes.added.insert("extra".into(), extra);
        save(&inspect(&source, &token).unwrap(), &changes, &token).unwrap();
        let mut archive = ZipArchive::new(File::open(source.as_path()).unwrap()).unwrap();
        assert_eq!(archive.comment(), b"Keep this comment");
        assert_eq!(
            archive.by_name("script.sh").unwrap().unix_mode().unwrap() & 0o777,
            0o755
        );
        let source = VPath::from(dir.path().join("archive.tar"));
        let mut archive = TarBuilder::new(File::create(source.as_path()).unwrap());
        let mut header = TarHeader::new_gnu();
        header.set_mode(0o777);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_cksum();
        archive.append_link(&mut header, "link", "target").unwrap();
        archive.finish().unwrap();
        save(&inspect(&source, &token).unwrap(), &changes, &token).unwrap();
        let mut archive = TarArchive::new(File::open(source.as_path()).unwrap());
        let entry = archive.entries().unwrap().next().unwrap().unwrap();
        assert!(entry.header().entry_type().is_symlink());
        assert_eq!(entry.link_name().unwrap().unwrap(), Path::new("target"));
    }
}
