use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use dualpane_core::{CancelToken, EntryKind, SizeHint, VPath};
use dualpane_vfs::Vfs;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sevenz_rust2::{
    ArchiveEntry as SevenZEntry, ArchiveReader as SevenZReader, ArchiveWriter as SevenZWriter,
    Password as SevenZPassword,
};
use tar::{Archive as TarArchive, Builder as TarBuilder, Header as TarHeader};
type FileOptions<'a> = zip::write::FileOptions<'a, ()>;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub mod edit;
mod password;
mod progress;
pub use password::Password;
pub use progress::{ArchiveProgress, ArchiveTask};

const BUFFER_SIZE: usize = 128 * 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    SevenZ,
}

impl ArchiveFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::SevenZ => "7z",
        }
    }
}

pub fn create_archive(
    vfs: &dyn Vfs,
    sources: &[VPath],
    destination: &VPath,
    format: ArchiveFormat,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    if task.password.is_some() && !matches!(format, ArchiveFormat::Zip | ArchiveFormat::SevenZ) {
        return Err("Password protection is available for ZIP and 7Z archives".into());
    }
    task.check()
        .map_err(|_| "Archive operation cancelled".to_owned())?;
    match vfs.stat(destination, false) {
        Ok(_) => {
            return Err(format!(
                "{destination} already exists; choose another archive name"
            ));
        }
        Err(error) if error.io_kind() == Some(io::ErrorKind::NotFound) => {}
        Err(error) => return Err(error.to_string()),
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "Choose an archive destination".to_owned())?;
    // The temporary output lives beside the final archive. Never let recursive
    // traversal include that growing output, including through a directory alias.
    let resolved_parent = vfs.canonicalize(&parent).unwrap_or_else(|_| parent.clone());
    for source in sources {
        task.check()
            .map_err(|_| "Archive operation cancelled".to_owned())?;
        let metadata = vfs.stat(source, false).map_err(|error| error.to_string())?;
        if metadata.kind == EntryKind::Directory {
            let resolved_source = vfs.canonicalize(source).unwrap_or_else(|_| source.clone());
            if resolved_parent
                .as_path()
                .starts_with(resolved_source.as_path())
            {
                return Err(format!("Choose an archive destination outside {source}"));
            }
        }
    }
    let temporary = parent.join_name(OsStr::new(&format!(
        ".commander-archive-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        dualpane_engine::JobId::next().get(),
    )));
    let writer = vfs
        .create_write(&temporary, SizeHint::Unknown)
        .map_err(|error| error.to_string())?;
    let identity = vfs
        .stat(&temporary, false)
        .map_err(|error| error.to_string())?
        .identity;
    let result = (|| {
        let count = match format {
            ArchiveFormat::Zip => create_zip(vfs, sources, writer, task)?,
            ArchiveFormat::Tar => {
                let (count, mut writer) = create_tar(vfs, sources, TarBuilder::new(writer), task)?;
                writer.flush().map_err(|error| error.to_string())?;
                count
            }
            ArchiveFormat::TarGz => {
                let encoder = GzEncoder::new(writer, Compression::default());
                let (count, encoder) = create_tar(vfs, sources, TarBuilder::new(encoder), task)?;
                let mut writer = encoder.finish().map_err(|error| error.to_string())?;
                writer.flush().map_err(|error| error.to_string())?;
                count
            }
            ArchiveFormat::SevenZ => create_sevenz(vfs, sources, writer, task)?,
        };
        task.finishing()?;
        vfs.sync_file(&temporary)
            .map_err(|error| error.to_string())?;
        task.check()
            .map_err(|_| "Archive operation cancelled".to_owned())?;
        vfs.rename_noreplace(&temporary, destination)
            .map_err(|error| error.to_string())?;
        vfs.sync_file(&parent).map_err(|error| error.to_string())?;
        Ok(count)
    })();
    if let Err(error) = result {
        if vfs
            .stat(&temporary, false)
            .is_ok_and(|metadata| metadata.identity == identity)
            && let Err(cleanup) = vfs.remove(&temporary, EntryKind::File)
        {
            return Err(format!(
                "{error}. Could not remove incomplete archive {temporary}: {cleanup}"
            ));
        }
        return Err(error);
    }
    result
}

fn create_sevenz(
    vfs: &dyn Vfs,
    sources: &[VPath],
    writer: Box<dyn dualpane_vfs::WriteSeek>,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    let mut archive = SevenZWriter::new(writer).map_err(|error| error.to_string())?;
    password::configure_sevenz(&mut archive, task.password.as_ref());
    let mut count = 0_usize;
    for source in sources {
        let Some(name) = source.file_name() else {
            return Err(format!("Cannot archive filesystem root {source}"));
        };
        count += add_sevenz_path(vfs, &mut archive, source, PathBuf::from(name), task)?;
    }
    task.finishing()?;
    archive
        .finish()
        .map_err(|error| error.to_string())?
        .flush()
        .map_err(|error| error.to_string())?;
    Ok(count)
}

fn add_sevenz_path(
    vfs: &dyn Vfs,
    archive: &mut SevenZWriter<Box<dyn dualpane_vfs::WriteSeek>>,
    source: &VPath,
    archive_path: PathBuf,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    task.check().map_err(|_| "Archive cancelled".to_owned())?;
    let metadata = vfs.stat(source, false).map_err(|error| error.to_string())?;
    task.begin(source);
    let name = archive_name(&archive_path)?;
    if metadata.kind == EntryKind::Directory {
        archive
            .push_archive_entry(SevenZEntry::new_directory(&name), None::<io::Empty>)
            .map_err(|error| error.to_string())?;
        task.item_done();
        let entries = vfs
            .read_dir(source, &task.cancel)
            .map_err(|error| error.to_string())?;
        let mut count = 1_usize;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            count += add_sevenz_path(
                vfs,
                archive,
                &source.join_name(entry.name()),
                archive_path.join(entry.name()),
                task,
            )?;
        }
        return Ok(count);
    }
    if metadata.kind != EntryKind::File {
        return Ok(0);
    }
    let reader = vfs.open_read(source).map_err(|error| error.to_string())?;
    archive
        .push_archive_entry(
            SevenZEntry::new_file(&name),
            Some(ProgressReader {
                inner: reader,
                task,
            }),
        )
        .map_err(|error| error.to_string())?;
    task.item_done();
    Ok(1)
}

fn create_zip(
    vfs: &dyn Vfs,
    sources: &[VPath],
    writer: Box<dyn dualpane_vfs::WriteSeek>,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    let mut archive = ZipWriter::new(writer);
    let password = task.password.clone();
    let options = password::zip_options(password.as_ref());
    let mut count = 0_usize;
    for source in sources {
        let Some(name) = source.file_name() else {
            return Err(format!("Cannot archive filesystem root {source}"));
        };
        count += add_zip_path(
            vfs,
            &mut archive,
            source,
            PathBuf::from(name),
            options,
            task,
        )?;
    }
    task.finishing()?;
    archive
        .finish()
        .map_err(|error| error.to_string())?
        .flush()
        .map_err(|error| error.to_string())?;
    Ok(count)
}

fn add_zip_path<W: Write + io::Seek>(
    vfs: &dyn Vfs,
    archive: &mut ZipWriter<W>,
    source: &VPath,
    archive_path: PathBuf,
    options: FileOptions<'_>,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    task.check().map_err(|_| "Archive cancelled".to_owned())?;
    let metadata = vfs.stat(source, false).map_err(|error| error.to_string())?;
    task.begin(source);
    let name = archive_name(&archive_path)?;
    if metadata.kind == EntryKind::Directory {
        archive
            .add_directory(format!("{name}/"), options)
            .map_err(|error| error.to_string())?;
        task.item_done();
        let entries = vfs
            .read_dir(source, &task.cancel)
            .map_err(|error| error.to_string())?;
        let mut count = 1_usize;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            count += add_zip_path(
                vfs,
                archive,
                &source.join_name(entry.name()),
                archive_path.join(entry.name()),
                options,
                task,
            )?;
        }
        return Ok(count);
    }
    if metadata.kind != EntryKind::File {
        return Ok(0);
    }
    archive
        .start_file(name, options)
        .map_err(|error| error.to_string())?;
    copy_reader(
        vfs.open_read(source).map_err(|error| error.to_string())?,
        archive,
        task,
    )?;
    task.item_done();
    Ok(1)
}

fn create_tar<W: Write>(
    vfs: &dyn Vfs,
    sources: &[VPath],
    mut archive: TarBuilder<W>,
    task: &mut ArchiveTask<'_>,
) -> Result<(usize, W), String> {
    let mut count = 0_usize;
    for source in sources {
        let Some(name) = source.file_name() else {
            return Err(format!("Cannot archive filesystem root {source}"));
        };
        count += add_tar_path(vfs, &mut archive, source, PathBuf::from(name), task)?;
    }
    task.finishing()?;
    archive.finish().map_err(|error| error.to_string())?;
    Ok((
        count,
        archive.into_inner().map_err(|error| error.to_string())?,
    ))
}

fn add_tar_path<W: Write>(
    vfs: &dyn Vfs,
    archive: &mut TarBuilder<W>,
    source: &VPath,
    archive_path: PathBuf,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    task.check().map_err(|_| "Archive cancelled".to_owned())?;
    let metadata = vfs.stat(source, false).map_err(|error| error.to_string())?;
    task.begin(source);
    let mut header = TarHeader::new_gnu();
    header.set_mode(metadata.mode.unwrap_or(0o644));
    header.set_mtime(
        metadata
            .modified
            .map_or(0, |value| value.seconds.max(0) as u64),
    );
    if metadata.kind == EntryKind::Directory {
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_cksum();
        archive
            .append_data(&mut header, &archive_path, std::io::empty())
            .map_err(|error| error.to_string())?;
        task.item_done();
        let entries = vfs
            .read_dir(source, &task.cancel)
            .map_err(|error| error.to_string())?;
        let mut count = 1_usize;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            count += add_tar_path(
                vfs,
                archive,
                &source.join_name(entry.name()),
                archive_path.join(entry.name()),
                task,
            )?;
        }
        return Ok(count);
    }
    if metadata.kind != EntryKind::File {
        return Ok(0);
    }
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(metadata.size);
    header.set_cksum();
    let reader = vfs.open_read(source).map_err(|error| error.to_string())?;
    archive
        .append_data(
            &mut header,
            &archive_path,
            ProgressReader {
                inner: reader,
                task,
            },
        )
        .map_err(|error| error.to_string())?;
    task.item_done();
    Ok(1)
}

pub fn extract_archive(
    vfs: &dyn Vfs,
    source: &VPath,
    destination: &VPath,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    password::unlock(vfs, source, task)?;
    let name = source
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".zip") {
        extract_zip(vfs, source, destination, task)
    } else if name.ends_with(".7z") {
        extract_sevenz(vfs, source, destination, task)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let reader = vfs.open_read(source).map_err(|error| error.to_string())?;
        extract_tar(vfs, GzDecoder::new(reader), destination, task)
    } else if name.ends_with(".tar") {
        let reader = vfs.open_read(source).map_err(|error| error.to_string())?;
        extract_tar(vfs, reader, destination, task)
    } else {
        Err("Supported archive formats are ZIP, 7Z, TAR, and TAR.GZ".to_owned())
    }
}

fn extract_sevenz(
    vfs: &dyn Vfs,
    source: &VPath,
    destination: &VPath,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    let reader = vfs.open_read(source).map_err(|error| error.to_string())?;
    let mut archive = SevenZReader::new(reader, password::sevenz_password(task.password.as_ref()))
        .map_err(|error| error.to_string())?;
    let mut count = 0_usize;
    archive
        .for_each_entries(|entry, reader| {
            task.check()
                .map_err(|_| sevenz_rust2::Error::from(io::Error::other("Extraction cancelled")))?;
            let relative = PathBuf::from(&entry.name);
            if !safe_relative_path(&relative) {
                return Err(sevenz_rust2::Error::from(io::Error::other(format!(
                    "Archive entry {} has an unsafe path",
                    entry.name
                ))));
            }
            let target = VPath::new(destination.as_path().join(&relative));
            task.begin(&target);
            if entry.is_directory {
                ensure_directory(vfs, &target)
                    .map_err(|error| sevenz_rust2::Error::from(io::Error::other(error)))?;
            } else {
                if let Some(parent) = target.parent() {
                    ensure_directory(vfs, &parent)
                        .map_err(|error| sevenz_rust2::Error::from(io::Error::other(error)))?;
                }
                let mut writer = vfs
                    .create_write(&target, SizeHint::Exact(entry.size))
                    .map_err(|error| {
                        sevenz_rust2::Error::from(io::Error::other(error.to_string()))
                    })?;
                copy_reader(reader, &mut writer, task)
                    .map_err(|error| sevenz_rust2::Error::from(io::Error::other(error)))?;
            }
            task.item_done();
            count = count.saturating_add(1);
            Ok(true)
        })
        .map_err(|error| error.to_string())?;
    Ok(count)
}

fn extract_zip(
    vfs: &dyn Vfs,
    source: &VPath,
    destination: &VPath,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    let reader = vfs.open_read(source).map_err(|error| error.to_string())?;
    let mut archive = ZipArchive::new(reader).map_err(|error| error.to_string())?;
    let mut count = 0_usize;
    for index in 0..archive.len() {
        task.check()
            .map_err(|_| "Extraction cancelled".to_owned())?;
        let mut entry = if let Some(password) = &task.password {
            archive.by_index_decrypt(index, password.expose().as_bytes())
        } else {
            archive.by_index(index)
        }
        .map_err(|error| error.to_string())?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(format!("Archive entry {} has an unsafe path", entry.name()));
        };
        let target = VPath::new(destination.as_path().join(relative));
        task.begin(&target);
        if entry.is_dir() {
            ensure_directory(vfs, &target)?;
        } else {
            if let Some(parent) = target.parent() {
                ensure_directory(vfs, &parent)?;
            }
            let mut writer = vfs
                .create_write(&target, SizeHint::Exact(entry.size()))
                .map_err(|error| error.to_string())?;
            copy_reader(&mut entry, &mut writer, task)?;
        }
        task.item_done();
        count = count.saturating_add(1);
    }
    Ok(count)
}

fn extract_tar<R: Read>(
    vfs: &dyn Vfs,
    reader: R,
    destination: &VPath,
    task: &mut ArchiveTask<'_>,
) -> Result<usize, String> {
    let mut archive = TarArchive::new(reader);
    let mut count = 0_usize;
    let entries = archive.entries().map_err(|error| error.to_string())?;
    for entry in entries {
        task.check()
            .map_err(|_| "Extraction cancelled".to_owned())?;
        let mut entry = entry.map_err(|error| error.to_string())?;
        let relative = entry
            .path()
            .map_err(|error| error.to_string())?
            .into_owned();
        if !safe_relative_path(&relative) {
            return Err(format!(
                "Archive entry {} has an unsafe path",
                relative.display()
            ));
        }
        let target = VPath::new(destination.as_path().join(&relative));
        task.begin(&target);
        if entry.header().entry_type().is_dir() {
            ensure_directory(vfs, &target)?;
        } else if entry.header().entry_type().is_file() {
            if let Some(parent) = target.parent() {
                ensure_directory(vfs, &parent)?;
            }
            let size = entry.header().size().unwrap_or(0);
            let mut writer = vfs
                .create_write(&target, SizeHint::Exact(size))
                .map_err(|error| error.to_string())?;
            copy_reader(&mut entry, &mut writer, task)?;
        }
        task.item_done();
        count = count.saturating_add(1);
    }
    Ok(count)
}

fn copy_reader(
    mut reader: impl Read,
    mut writer: impl Write,
    task: &mut ArchiveTask<'_>,
) -> Result<u64, String> {
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        task.check()
            .map_err(|_| "Archive operation cancelled".to_owned())?;
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        writer
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
        total = total.saturating_add(count as u64);
        task.advanced(count as u64);
    }
    Ok(total)
}

struct ProgressReader<'a, 'b, R> {
    inner: R,
    task: &'a mut ArchiveTask<'b>,
}

impl<R: Read> Read for ProgressReader<'_, '_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.task
            .check()
            .map_err(|_| io::Error::other("Archive operation cancelled"))?;
        let length = buffer.len().min(BUFFER_SIZE);
        let count = self.inner.read(&mut buffer[..length])?;
        self.task.advanced(count as u64);
        Ok(count)
    }
}

fn ensure_directory(vfs: &dyn Vfs, path: &VPath) -> Result<(), String> {
    if vfs.stat(path, false).is_ok() {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && parent != *path
    {
        ensure_directory(vfs, &parent)?;
    }
    vfs.create_dir(path).map_err(|error| error.to_string())
}

fn safe_relative_path(path: &Path) -> bool {
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn archive_name(path: &Path) -> Result<String, String> {
    if !safe_relative_path(path) {
        return Err(format!("Unsafe archive path: {}", path.display()));
    }
    Ok(path.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dualpane_core::{CancelToken, VPath};
    use dualpane_vfs::LocalFs;

    use super::{ArchiveFormat, ArchiveTask, create_archive, extract_archive};

    #[test]
    fn every_format_reports_progress_and_round_trips() {
        for format in [
            ArchiveFormat::Zip,
            ArchiveFormat::Tar,
            ArchiveFormat::TarGz,
            ArchiveFormat::SevenZ,
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let source = fixture.path().join("source");
            let output = fixture.path().join("out");
            fs::create_dir(&source).unwrap();
            fs::create_dir(&output).unwrap();
            let payload = vec![42_u8; super::BUFFER_SIZE * 3 + 17];
            fs::write(source.join("payload.bin"), &payload).unwrap();
            fs::write(source.join("empty"), []).unwrap();
            let destination = VPath::from(
                fixture
                    .path()
                    .join(format!("test.{}", format.extension()))
                    .as_path(),
            );
            let control = dualpane_engine::JobControl::new();
            let mut events = Vec::new();
            let count = {
                let mut task =
                    ArchiveTask::with_progress(&control, |progress| events.push(progress));
                let count = create_archive(
                    &LocalFs,
                    &[VPath::from(source.as_path())],
                    &destination,
                    format,
                    &mut task,
                )
                .unwrap();
                task.flush();
                count
            };
            assert_eq!(count, 3, "{format:?}");
            assert!(events.first().unwrap().current_path.is_some());
            assert_eq!(
                events.last().unwrap().bytes_done,
                payload.len() as u64,
                "{format:?}"
            );
            assert_eq!(events.last().unwrap().items_done, 3);
            assert!(events.last().unwrap().finishing);
            let mut extracted = Vec::new();
            {
                let mut task =
                    ArchiveTask::with_progress(&control, |progress| extracted.push(progress));
                extract_archive(
                    &LocalFs,
                    &destination,
                    &VPath::from(output.as_path()),
                    &mut task,
                )
                .unwrap();
                task.flush();
            }
            assert_eq!(
                extracted.last().unwrap().bytes_done,
                payload.len() as u64,
                "{format:?}"
            );
            assert_eq!(extracted.last().unwrap().items_done, 3);
            assert_eq!(
                fs::read(output.join("source/payload.bin")).unwrap(),
                payload
            );
        }
    }

    #[test]
    fn archive_destination_cannot_be_inside_a_source_folder() {
        for format in [
            ArchiveFormat::Zip,
            ArchiveFormat::Tar,
            ArchiveFormat::TarGz,
            ArchiveFormat::SevenZ,
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let source = fixture.path().join("source");
            let nested = source.join("nested");
            fs::create_dir_all(&nested).unwrap();
            fs::write(source.join("payload"), b"keep me").unwrap();
            let mut parents = vec![source.clone(), nested.clone()];
            #[cfg(unix)]
            {
                let alias = fixture.path().join("alias");
                std::os::unix::fs::symlink(&nested, &alias).unwrap();
                parents.push(alias);
            }
            for parent in parents {
                let destination = parent.join(format!("backup.{}", format.extension()));
                let error = create_archive(
                    &LocalFs,
                    &[VPath::from(source.as_path())],
                    &VPath::from(destination.as_path()),
                    format,
                    &mut ArchiveTask::new(&CancelToken::new()),
                )
                .unwrap_err();
                assert!(error.contains("outside"), "{format:?}: {error}");
                assert!(!destination.exists());
                assert_eq!(fs::read_dir(&nested).unwrap().count(), 0);
                assert_eq!(fs::read_dir(&source).unwrap().count(), 2);
                assert_eq!(fs::read(source.join("payload")).unwrap(), b"keep me");
            }
        }
    }

    #[test]
    fn cancellation_removes_partial_archives_and_keeps_existing_destinations() {
        for format in [
            ArchiveFormat::Zip,
            ArchiveFormat::Tar,
            ArchiveFormat::TarGz,
            ArchiveFormat::SevenZ,
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let source = fixture.path().join("payload");
            fs::write(&source, vec![1; super::BUFFER_SIZE * 2]).unwrap();
            let destination = fixture.path().join(format!("test.{}", format.extension()));
            let control = dualpane_engine::JobControl::new();
            let mut task = ArchiveTask::with_progress(&control, |_| control.cancel());
            assert!(
                create_archive(
                    &LocalFs,
                    &[VPath::from(source.as_path())],
                    &VPath::from(destination.as_path()),
                    format,
                    &mut task
                )
                .is_err()
            );
            assert!(!destination.exists(), "{format:?}");
            assert_eq!(
                fs::read_dir(fixture.path()).unwrap().count(),
                1,
                "cancelled temporary output left behind: {format:?}"
            );
            fs::write(&destination, b"original archive").unwrap();
            let mut task = ArchiveTask::new(&CancelToken::new());
            assert!(
                create_archive(
                    &LocalFs,
                    &[VPath::from(source.as_path())],
                    &VPath::from(destination.as_path()),
                    format,
                    &mut task
                )
                .is_err()
            );
            assert_eq!(fs::read(&destination).unwrap(), b"original archive");
            assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 2);
        }
    }

    #[test]
    fn archive_readers_pause_and_cancel_between_chunks() {
        use std::io::Read;
        use std::time::Duration;
        let control = dualpane_engine::JobControl::new();
        control.pause();
        let worker_control = control.clone();
        let (ready, entered) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut task = ArchiveTask::with_progress(&worker_control, |_| {});
            let mut reader = super::ProgressReader {
                inner: &b"payload"[..],
                task: &mut task,
            };
            ready.send(()).unwrap();
            done.send(reader.read(&mut [0; 7])).unwrap();
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            finished.recv_timeout(Duration::from_millis(80)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        control.cancel();
        assert!(
            finished
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
        worker.join().unwrap();
    }

    #[test]
    fn zip_round_trip_stays_inside_the_vfs() {
        let fixture = tempfile::tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        fs::create_dir_all(source.join("nested")).expect("source tree");
        fs::create_dir_all(&destination).expect("destination");
        fs::write(source.join("nested/file.txt"), "archive payload").expect("payload");
        let archive = fixture.path().join("sample.zip");
        let cancel = CancelToken::new();

        create_archive(
            &LocalFs,
            &[VPath::from(source.as_path())],
            &VPath::from(archive.as_path()),
            ArchiveFormat::Zip,
            &mut ArchiveTask::new(&cancel),
        )
        .expect("create archive");
        extract_archive(
            &LocalFs,
            &VPath::from(archive.as_path()),
            &VPath::from(destination.as_path()),
            &mut ArchiveTask::new(&cancel),
        )
        .expect("extract archive");

        assert_eq!(
            fs::read_to_string(destination.join("source/nested/file.txt")).expect("read result"),
            "archive payload"
        );
    }

    #[test]
    fn sevenz_round_trip_stays_inside_the_vfs() {
        let fixture = tempfile::tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        fs::create_dir_all(source.join("nested")).expect("source tree");
        fs::create_dir_all(&destination).expect("destination");
        fs::write(source.join("nested/file.txt"), "7z payload").expect("payload");
        let archive = fixture.path().join("sample.7z");
        let cancel = CancelToken::new();

        create_archive(
            &LocalFs,
            &[VPath::from(source.as_path())],
            &VPath::from(archive.as_path()),
            ArchiveFormat::SevenZ,
            &mut ArchiveTask::new(&cancel),
        )
        .expect("create archive");
        extract_archive(
            &LocalFs,
            &VPath::from(archive.as_path()),
            &VPath::from(destination.as_path()),
            &mut ArchiveTask::new(&cancel),
        )
        .expect("extract archive");

        assert_eq!(
            fs::read_to_string(destination.join("source/nested/file.txt")).expect("read result"),
            "7z payload"
        );
    }
}
