//! Deterministic failure injection shared by transfer and synchronization tests.
use dualpane_core::{CancelToken, Capabilities, Entry, EntryKind, Metadata, SizeHint, VPath};
use dualpane_vfs::{LocalFs, ReadSeek, Result, TrashLocation, Vfs, VfsError, WriteSeek};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug)]
pub enum Failure {
    DiskFull,
    Disconnected,
    Flush,
    Sync,
    Cancel,
    Crash,
}

pub struct FaultFs {
    pub source: VPath,
    pub failure: Option<Failure>,
    pub trash: PathBuf,
    pub cross_device: bool,
    pub nonseekable: bool,
    pub cancel: CancelToken,
    pub triggered: Arc<AtomicBool>,
}
impl FaultFs {
    pub fn new(source: VPath, trash: PathBuf) -> Self {
        Self {
            source,
            failure: None,
            trash,
            cross_device: false,
            nonseekable: false,
            cancel: CancelToken::new(),
            triggered: Arc::new(AtomicBool::new(false)),
        }
    }
    fn writer(&self, inner: Box<dyn WriteSeek>) -> Box<dyn WriteSeek> {
        Box::new(FaultWriter {
            inner,
            failure: self.failure,
            written: 0,
            cancel: self.cancel.clone(),
            triggered: Arc::clone(&self.triggered),
        })
    }
}
fn error(path: &VPath, code: i32) -> VfsError {
    VfsError::Io {
        operation: "injected failure",
        path: path.clone(),
        source: io::Error::from_raw_os_error(code),
    }
}
impl Vfs for FaultFs {
    fn read_dir(
        &self,
        p: &VPath,
        c: &CancelToken,
    ) -> Result<Box<dyn Iterator<Item = Result<Entry>> + Send>> {
        LocalFs.read_dir(p, c)
    }
    fn stat(&self, p: &VPath, f: bool) -> Result<Metadata> {
        LocalFs.stat(p, f)
    }
    fn open_read(&self, p: &VPath) -> Result<Box<dyn ReadSeek>> {
        let inner = LocalFs.open_read(p)?;
        if p == &self.source
            && (self.nonseekable || matches!(self.failure, Some(Failure::Disconnected)))
        {
            Ok(Box::new(FaultReader {
                inner,
                read: 0,
                nonseekable: self.nonseekable,
                disconnected: matches!(self.failure, Some(Failure::Disconnected)),
                triggered: Arc::clone(&self.triggered),
            }))
        } else {
            Ok(inner)
        }
    }
    fn create_write(&self, p: &VPath, h: SizeHint) -> Result<Box<dyn WriteSeek>> {
        Ok(self.writer(LocalFs.create_write(p, h)?))
    }
    fn open_write(&self, p: &VPath) -> Result<Box<dyn WriteSeek>> {
        Ok(self.writer(LocalFs.open_write(p)?))
    }
    fn create_dir(&self, p: &VPath) -> Result<()> {
        LocalFs.create_dir(p)
    }
    fn rename(&self, a: &VPath, b: &VPath) -> Result<()> {
        if self.cross_device && a == &self.source {
            Err(error(a, 18))
        } else {
            LocalFs.rename(a, b)
        }
    }
    fn rename_noreplace(&self, a: &VPath, b: &VPath) -> Result<()> {
        if self.cross_device && a == &self.source {
            Err(error(a, 18))
        } else {
            LocalFs.rename_noreplace(a, b)
        }
    }
    fn remove(&self, p: &VPath, k: EntryKind) -> Result<()> {
        LocalFs.remove(p, k)
    }
    fn capabilities(&self) -> Capabilities {
        LocalFs.capabilities()
    }
    fn canonicalize(&self, p: &VPath) -> Result<VPath> {
        LocalFs.canonicalize(p)
    }
    fn read_link(&self, p: &VPath) -> Result<PathBuf> {
        LocalFs.read_link(p)
    }
    fn create_symlink(&self, a: &VPath, b: &VPath) -> Result<()> {
        LocalFs.create_symlink(a, b)
    }
    fn preserve_metadata(&self, s: &VPath, d: &VPath, m: &Metadata) -> Result<Vec<String>> {
        LocalFs.preserve_metadata(s, d, m)
    }
    fn sync_file(&self, p: &VPath) -> Result<()> {
        if matches!(self.failure, Some(Failure::Sync)) {
            self.triggered.store(true, Ordering::SeqCst);
            Err(error(p, 5))
        } else {
            LocalFs.sync_file(p)
        }
    }
    fn set_len(&self, p: &VPath, n: u64) -> Result<()> {
        LocalFs.set_len(p, n)
    }
    fn prepare_trash(&self, _: &VPath) -> Result<TrashLocation> {
        let files = self.trash.join("files");
        let info = self.trash.join("info");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::create_dir_all(&info).unwrap();
        Ok(TrashLocation {
            files: files.into(),
            info: info.into(),
            deletion_date: "2026-09-08T00:00:00".into(),
        })
    }
}
struct FaultWriter {
    inner: Box<dyn WriteSeek>,
    failure: Option<Failure>,
    written: usize,
    cancel: CancelToken,
    triggered: Arc<AtomicBool>,
}
impl Seek for FaultWriter {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        self.inner.seek(p)
    }
}
impl Write for FaultWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.written >= 4096 {
            match self.failure {
                Some(Failure::DiskFull) => {
                    self.triggered.store(true, Ordering::SeqCst);
                    return Err(io::Error::from_raw_os_error(28));
                }
                Some(Failure::Crash) => std::process::exit(77),
                Some(Failure::Cancel) => {
                    self.triggered.store(true, Ordering::SeqCst);
                    self.cancel.cancel();
                }
                _ => {}
            }
        }
        let count = self.inner.write(&bytes[..bytes.len().min(4096)])?;
        self.written += count;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        if matches!(self.failure, Some(Failure::Flush)) {
            self.triggered.store(true, Ordering::SeqCst);
            Err(io::Error::from_raw_os_error(5))
        } else {
            self.inner.flush()
        }
    }
}
struct FaultReader {
    inner: Box<dyn ReadSeek>,
    read: usize,
    nonseekable: bool,
    disconnected: bool,
    triggered: Arc<AtomicBool>,
}
impl Seek for FaultReader {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        if self.nonseekable {
            Err(io::Error::from_raw_os_error(29))
        } else {
            self.inner.seek(p)
        }
    }
}
impl Read for FaultReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.disconnected && self.read >= 4096 {
            self.triggered.store(true, Ordering::SeqCst);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "connection lost during transfer",
            ));
        }
        let limit = bytes.len().min(4096);
        let count = self.inner.read(&mut bytes[..limit])?;
        self.read += count;
        Ok(count)
    }
}
