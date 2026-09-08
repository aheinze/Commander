//! Archive transactions stream unchanged entries and publish only complete output.
use super::*;
use dualpane_vfs::LocalFs;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
};

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub size: u64,
    pub directory: bool,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub source: VPath,
    pub format: ArchiveFormat,
    pub digest: String,
    pub items: Vec<Item>,
}
#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub removed: BTreeSet<String>,
    pub added: BTreeMap<String, PathBuf>,
}
impl Changes {
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
pub fn inspect(source: &VPath, cancel: &CancelToken) -> Result<Snapshot, String> {
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
            })
            .map_err(|e| e.to_string())?;
            if archive.len() > 100_000 {
                return Err("Archive exceeds 100,000 entries".into());
            }
            for index in 0..archive.len() {
                cancel
                    .check()
                    .map_err(|_| "Archive scan cancelled".to_owned())?;
                let entry = archive.by_index(index).map_err(|e| e.to_string())?;
                items.push(Item {
                    name: checked_name(entry.name())?,
                    size: entry.size(),
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
                    size: entry.size(),
                    directory: entry.header().entry_type().is_dir(),
                });
            }
        }
        ArchiveFormat::SevenZ => {
            let archive = SevenZReader::new(
                Cancellable {
                    inner: file,
                    cancel,
                },
                SevenZPassword::empty(),
            )
            .map_err(|e| e.to_string())?;
            for entry in &archive.archive().files {
                cancel
                    .check()
                    .map_err(|_| "Archive scan cancelled".to_owned())?;
                items.push(Item {
                    name: checked_name(&entry.name)?,
                    size: entry.size,
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
    })
}

pub fn save(
    snapshot: &Snapshot,
    changes: &Changes,
    cancel: &CancelToken,
) -> Result<PathBuf, String> {
    if changes.removed.is_empty() && changes.added.is_empty() {
        return Err("No archive changes to save".into());
    }
    for name in changes.removed.iter().chain(changes.added.keys()) {
        if checked_name(name)? != *name {
            return Err("Use normalized archive paths without './' or repeated separators".into());
        }
    }
    let mut final_items: BTreeMap<String, bool> = snapshot
        .items
        .iter()
        .filter(|item| !changes.excludes(&item.name))
        .map(|item| (item.name.clone(), item.directory))
        .collect();
    for (name, source) in &changes.added {
        let metadata =
            fs::symlink_metadata(source).map_err(|e| format!("{}: {e}", source.display()))?;
        if !metadata.is_file() {
            return Err(
                "Only regular files can be added. Add a folder’s files individually.".into(),
            );
        }
        final_items.insert(name.clone(), false);
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
            "Archive changed since opening. Close this editor and open it again before saving."
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
    let mut task = ArchiveTask::new(cancel);
    match snapshot.format {
        ArchiveFormat::Zip => {
            let mut input = ZipArchive::new(Cancellable {
                inner: file,
                cancel,
            })
            .map_err(|e| e.to_string())?;
            let mut writer = ZipWriter::new(output.as_file_mut());
            writer.set_raw_comment(input.comment().to_vec());
            for index in 0..input.len() {
                task.check()
                    .map_err(|_| "Archive save cancelled".to_owned())?;
                let entry = input.by_index(index).map_err(|e| e.to_string())?;
                if !changes.excludes(&checked_name(entry.name())?) {
                    writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
                }
            }
            for (name, source) in &changes.added {
                add_zip_path(
                    &LocalFs,
                    &mut writer,
                    &VPath::from(source.as_path()),
                    PathBuf::from(name),
                    FileOptions::default().compression_method(CompressionMethod::Deflated),
                    &mut task,
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
                rewrite_tar(input, &mut writer, changes, &mut task)?;
                writer
                    .into_inner()
                    .map_err(|e| e.to_string())?
                    .finish()
                    .map_err(|e| e.to_string())?;
            } else {
                let mut writer = TarBuilder::new(output.as_file_mut());
                rewrite_tar(input, &mut writer, changes, &mut task)?;
                writer.finish().map_err(|e| e.to_string())?;
            }
        }
        ArchiveFormat::SevenZ => {
            let mut reader = SevenZReader::new(
                Cancellable {
                    inner: file,
                    cancel,
                },
                SevenZPassword::empty(),
            )
            .map_err(|e| e.to_string())?;
            let mut writer = SevenZWriter::new(output.as_file_mut()).map_err(|e| e.to_string())?;
            reader
                .for_each_entries(|entry, reader| {
                    let failure = |e: String| sevenz_rust2::Error::from(io::Error::other(e));
                    if !changes.excludes(&checked_name(&entry.name).map_err(failure)?) {
                        let mut preserved = entry.clone();
                        // sevenz-rust2 0.20.2 writes the complement of the anti-item bit
                        // for streamless entries. Compensate so the original bit survives.
                        if !preserved.has_stream {
                            preserved.is_anti_item = !preserved.is_anti_item;
                        }
                        writer.push_archive_entry(
                            preserved,
                            Some(Cancellable {
                                inner: reader,
                                cancel,
                            }),
                        )?;
                    } else {
                        io::copy(
                            &mut Cancellable {
                                inner: reader,
                                cancel,
                            },
                            &mut io::sink(),
                        )
                        .map_err(sevenz_rust2::Error::from)?;
                    }
                    Ok(true)
                })
                .map_err(|e| e.to_string())?;
            for (name, source) in &changes.added {
                let file = File::open(source).map_err(|e| e.to_string())?;
                writer
                    .push_archive_entry(
                        SevenZEntry::new_file(name),
                        Some(Cancellable {
                            inner: file,
                            cancel,
                        }),
                    )
                    .map_err(|e| e.to_string())?;
            }
            writer.finish().map_err(|e| e.to_string())?;
        }
    }
    task.check()
        .map_err(|_| "Archive save cancelled".to_owned())?;
    let permissions = fs::metadata(snapshot.source.as_path())
        .map_err(|e| e.to_string())?
        .permissions();
    output
        .as_file()
        .set_permissions(permissions)
        .map_err(|e| e.to_string())?;
    output.as_file().sync_all().map_err(|e| e.to_string())?;
    // A durable recovery copy also detects a file modified during compression.
    let mut backup = tempfile::Builder::new()
        .prefix(".commander-archive-backup-")
        .suffix(&format!(".{}", snapshot.format.extension()))
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    let input = File::open(snapshot.source.as_path()).map_err(|e| e.to_string())?;
    io::copy(
        &mut Cancellable {
            inner: input,
            cancel,
        },
        &mut backup,
    )
    .map_err(|e| e.to_string())?;
    backup.as_file().sync_all().map_err(|e| e.to_string())?;
    if crate::features::sha256(&LocalFs, &VPath::from(backup.path()), cancel)? != snapshot.digest
        || crate::features::sha256(&LocalFs, &snapshot.source, cancel)? != snapshot.digest
    {
        return Err("Archive changed during save. Original archive was not replaced.".into());
    }
    task.check()
        .map_err(|_| "Archive save cancelled".to_owned())?;
    let (_, backup_path) = backup.keep().map_err(|e| e.to_string())?;
    output.persist(snapshot.source.as_path()).map_err(|e| {
        format!(
            "Could not publish archive: {e}. Recovery copy: {}",
            backup_path.display()
        )
    })?;
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|e| {
            format!(
                "Archive saved, but folder sync failed: {e}. Recovery copy: {}",
                backup_path.display()
            )
        })?;
    Ok(backup_path)
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
    })
    .entries()
    .map_err(|e| e.to_string())?
    {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if root_directory(&path, entry.header())
            || !changes.excludes(&checked_name(path.to_str().ok_or("Invalid archive name")?)?)
        {
            let mut header = entry.header().clone();
            writer
                .append_data(&mut header, &path, &mut entry)
                .map_err(|e| e.to_string())?;
        }
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
}
impl<R: Read> Read for Cancellable<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.cancel
            .check()
            .map_err(|_| io::Error::other("Archive operation cancelled"))?;
        let length = buffer.len().min(BUFFER_SIZE);
        self.inner.read(&mut buffer[..length])
    }
}

impl<R: io::Seek> io::Seek for Cancellable<'_, R> {
    fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
        self.cancel
            .check()
            .map_err(|_| io::Error::other("Archive operation cancelled"))?;
        self.inner.seek(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        zip.set_comment("Keep this comment");
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
