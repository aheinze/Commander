#![forbid(unsafe_code)]

//! Cancellable native thumbnail and preview loading.

pub mod markdown;
pub mod pdf;
mod svg;
mod syntax;
pub mod table;

pub use syntax::{SyntaxKind, SyntaxSpan};

use std::ffi::OsStr;
use std::io::{BufReader, Read};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, unbounded};
pub use dualpane_core::{CancelToken, Cancelled};
use dualpane_core::{EntryKind, FileIdentity, Metadata, Timestamp, VPath};
use dualpane_vfs::{Vfs, VfsError};
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder};
use thiserror::Error;

const MAX_TEXT_BYTES: usize = 512 * 1_024;
const MAX_IMAGE_EDGE: u32 = 1_600;
const MAX_MEDIA_BYTES: usize = 128 * 1_024 * 1_024;
const MAX_THUMBNAIL_SOURCE_EDGE: u32 = 32_768;
const MAX_THUMBNAIL_DECODE_BYTES: u64 = 256 * 1_024 * 1_024;
const DEFAULT_THUMBNAIL_WORKERS: usize = 2;
const DEFAULT_THUMBNAIL_QUEUE: usize = 512;

/// Source attributes used to reject a stale cached thumbnail.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThumbnailFingerprint {
    pub identity: Option<FileIdentity>,
    pub size: u64,
    pub modified: Option<Timestamp>,
}

impl ThumbnailFingerprint {
    /// Creates a cache fingerprint from filesystem metadata.
    #[must_use]
    pub const fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            identity: metadata.identity,
            size: metadata.size,
            modified: metadata.modified,
        }
    }
}

/// A bounded RGBA thumbnail decoded away from the UI thread.
#[derive(Clone, Debug)]
pub struct Thumbnail {
    pub rgba: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
    pub fingerprint: ThumbnailFingerprint,
}

/// Work submitted to the shared thumbnail pool.
#[derive(Clone, Debug)]
pub struct ThumbnailRequest {
    pub id: u64,
    pub path: VPath,
    pub max_edge: u32,
    pub cancel: CancelToken,
}

/// Completed thumbnail work returned by the shared pool.
#[derive(Debug)]
pub struct ThumbnailResponse {
    pub id: u64,
    pub path: VPath,
    pub result: Result<Thumbnail, ThumbnailError>,
}

#[derive(Debug, Error)]
pub enum ThumbnailError {
    #[error("thumbnail was cancelled")]
    Cancelled,
    #[error("file format does not support thumbnails")]
    Unsupported,
    #[error(transparent)]
    Vfs(#[from] VfsError),
    #[error("could not decode thumbnail: {0}")]
    Image(#[from] image::ImageError),
    #[error("could not read thumbnail: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ThumbnailScheduleError {
    #[error("thumbnail queue is full")]
    Full,
    #[error("thumbnail service has stopped")]
    Stopped,
}

/// Fixed-size thumbnail worker pool with a bounded request queue.
pub struct ThumbnailScheduler {
    requests: Option<Sender<ThumbnailRequest>>,
    results: Receiver<ThumbnailResponse>,
    workers: Vec<JoinHandle<()>>,
}

impl ThumbnailScheduler {
    /// Starts a small pool sized for interactive image decoding.
    pub fn new(vfs: Arc<dyn Vfs>) -> std::io::Result<Self> {
        let workers = thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(2, DEFAULT_THUMBNAIL_WORKERS);
        Self::with_capacity(vfs, workers, DEFAULT_THUMBNAIL_QUEUE)
    }

    fn with_capacity(
        vfs: Arc<dyn Vfs>,
        worker_count: usize,
        queue_capacity: usize,
    ) -> std::io::Result<Self> {
        let (requests, pending) = bounded::<ThumbnailRequest>(queue_capacity.max(1));
        let (completed, results) = unbounded::<ThumbnailResponse>();
        let mut workers = Vec::with_capacity(worker_count.max(1));
        for index in 0..worker_count.max(1) {
            let worker_pending = pending.clone();
            let worker_completed = completed.clone();
            let vfs = Arc::clone(&vfs);
            match thread::Builder::new()
                .name(format!("dualpane-thumbnail-{index}"))
                .spawn(move || thumbnail_worker(vfs, &worker_pending, &worker_completed))
            {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    drop(requests);
                    drop(pending);
                    drop(completed);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(error);
                }
            }
        }
        drop(completed);
        Ok(Self {
            requests: Some(requests),
            results,
            workers,
        })
    }

    /// Returns a receiver that can be bridged to the application's main loop.
    #[must_use]
    pub fn results(&self) -> Receiver<ThumbnailResponse> {
        self.results.clone()
    }

    /// Enqueues work without ever blocking the caller.
    pub fn try_schedule(&self, request: ThumbnailRequest) -> Result<(), ThumbnailScheduleError> {
        let Some(requests) = &self.requests else {
            return Err(ThumbnailScheduleError::Stopped);
        };
        requests.try_send(request).map_err(|error| match error {
            TrySendError::Full(_) => ThumbnailScheduleError::Full,
            TrySendError::Disconnected(_) => ThumbnailScheduleError::Stopped,
        })
    }

    fn shutdown(&mut self) {
        self.requests.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for ThumbnailScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn thumbnail_worker(
    vfs: Arc<dyn Vfs>,
    pending: &Receiver<ThumbnailRequest>,
    completed: &Sender<ThumbnailResponse>,
) {
    while let Ok(request) = pending.recv() {
        let result = load_thumbnail(
            vfs.as_ref(),
            &request.path,
            request.max_edge,
            &request.cancel,
        );
        let response = ThumbnailResponse {
            id: request.id,
            path: request.path,
            result,
        };
        if completed.send(response).is_err() {
            break;
        }
    }
}

/// Returns whether the built-in decoder supports a path's image extension.
#[must_use]
pub fn supports_thumbnail(path: &VPath) -> bool {
    is_image(path.as_path().extension())
}

/// Loads one size-bounded, orientation-correct thumbnail.
///
/// This function is synchronous and must only be called from a worker thread.
pub fn load_thumbnail(
    vfs: &dyn Vfs,
    path: &VPath,
    max_edge: u32,
    cancel: &CancelToken,
) -> Result<Thumbnail, ThumbnailError> {
    cancel.check().map_err(|_| ThumbnailError::Cancelled)?;
    if !supports_thumbnail(path) {
        return Err(ThumbnailError::Unsupported);
    }
    let metadata = vfs.stat(path, false)?;
    if metadata.kind != EntryKind::File {
        return Err(ThumbnailError::Unsupported);
    }
    let fingerprint = ThumbnailFingerprint::from_metadata(&metadata);
    let reader = BufReader::new(vfs.open_read(path)?);
    let mut reader = image::ImageReader::new(reader).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_THUMBNAIL_SOURCE_EDGE);
    limits.max_image_height = Some(MAX_THUMBNAIL_SOURCE_EDGE);
    limits.max_alloc = Some(MAX_THUMBNAIL_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    cancel.check().map_err(|_| ThumbnailError::Cancelled)?;
    image.apply_orientation(orientation);
    let max_edge = max_edge.clamp(32, 512);
    let image = if image.width() > max_edge || image.height() > max_edge {
        image.resize(max_edge, max_edge, FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    cancel.check().map_err(|_| ThumbnailError::Cancelled)?;
    Ok(Thumbnail {
        rgba: Arc::from(rgba.into_raw()),
        width,
        height,
        fingerprint,
    })
}

/// Decoded preview data that can cross from a worker to the GTK thread.
#[derive(Clone, Debug)]
pub enum PreviewPayload {
    Directory,
    Table(table::TableDocument),
    Markdown {
        document: markdown::MarkdownDocument,
        truncated: bool,
    },
    Image {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
    Text {
        content: String,
        language: &'static str,
        truncated: bool,
        highlights: Vec<SyntaxSpan>,
    },
    Pdf(pdf::PdfDocument),
    Media {
        bytes: Vec<u8>,
        kind: &'static str,
    },
    Unsupported,
}

/// Metadata and renderable content for one path.
#[derive(Clone, Debug)]
pub struct Preview {
    pub path: VPath,
    pub metadata: Metadata,
    pub payload: PreviewPayload,
}

#[derive(Debug, Error)]
pub enum PreviewError {
    #[error("preview was cancelled")]
    Cancelled,
    #[error(transparent)]
    Vfs(#[from] VfsError),
    #[error("could not decode image: {0}")]
    Image(#[from] image::ImageError),
    #[error("could not read preview: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not render PDF: {0}")]
    Pdf(String),
    #[error("could not render SVG: {0}")]
    Svg(String),
    #[error("could not preview spreadsheet: {0}")]
    Table(String),
}

/// Loads metadata and decodes bounded preview content.
///
/// This function is synchronous by design and must be called on a worker thread.
pub fn load_preview(
    vfs: &dyn Vfs,
    path: VPath,
    cancel: &CancelToken,
) -> Result<Preview, PreviewError> {
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    let metadata = vfs.stat(&path, false)?;
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    let payload = if metadata.kind == EntryKind::Directory {
        PreviewPayload::Directory
    } else if metadata.kind == EntryKind::File && table::supports(path.as_path()) {
        PreviewPayload::Table(table::load(vfs, &path, cancel)?)
    } else if metadata.kind == EntryKind::File && markdown::is_markdown(path.as_path()) {
        match read_text(vfs, &path, cancel)? {
            Some((content, truncated)) => {
                let mut document = markdown::MarkdownDocument::parse(&content, cancel)?;
                document.load_images(vfs, &path, cancel)?;
                let truncated = truncated || document.truncated;
                PreviewPayload::Markdown {
                    document,
                    truncated,
                }
            }
            None => PreviewPayload::Unsupported,
        }
    } else if metadata.kind == EntryKind::File && svg::is_svg(path.as_path()) {
        svg::load(vfs, &path, cancel)?
    } else if metadata.kind == EntryKind::File && is_image(path.as_path().extension()) {
        load_image(vfs, &path, cancel)?
    } else if metadata.kind == EntryKind::File && is_pdf(path.as_path().extension()) {
        load_pdf(vfs, &path, cancel)?
    } else if metadata.kind == EntryKind::File && is_media(path.as_path().extension()) {
        load_media(vfs, &path, cancel)?
    } else if metadata.kind == EntryKind::File
        && let Some(text_syntax) = syntax::for_path(path.as_path())
    {
        load_text(vfs, &path, text_syntax, cancel)?
    } else {
        PreviewPayload::Unsupported
    };
    Ok(Preview {
        path,
        metadata,
        payload,
    })
}

fn load_pdf(
    vfs: &dyn Vfs,
    path: &VPath,
    cancel: &CancelToken,
) -> Result<PreviewPayload, PreviewError> {
    let mut reader = vfs.open_read(path)?;
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((MAX_MEDIA_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    if bytes.len() > MAX_MEDIA_BYTES {
        return Ok(PreviewPayload::Unsupported);
    }
    Ok(PreviewPayload::Pdf(pdf::PdfDocument::load(bytes, cancel)?))
}

fn load_media(
    vfs: &dyn Vfs,
    path: &VPath,
    cancel: &CancelToken,
) -> Result<PreviewPayload, PreviewError> {
    let mut reader = vfs.open_read(path)?;
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((MAX_MEDIA_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    if bytes.len() > MAX_MEDIA_BYTES {
        return Ok(PreviewPayload::Unsupported);
    }
    let extension = path
        .as_path()
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kind = if matches!(
        extension.as_str(),
        "mp3" | "m4a" | "flac" | "ogg" | "opus" | "wav"
    ) {
        "Audio"
    } else {
        "Video"
    };
    Ok(PreviewPayload::Media { bytes, kind })
}

fn load_image(
    vfs: &dyn Vfs,
    path: &VPath,
    cancel: &CancelToken,
) -> Result<PreviewPayload, PreviewError> {
    let reader = BufReader::new(vfs.open_read(path)?);
    let image = image::ImageReader::new(reader)
        .with_guessed_format()?
        .decode()?;
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    let image = if image.width() > MAX_IMAGE_EDGE || image.height() > MAX_IMAGE_EDGE {
        image.resize(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE, FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(PreviewPayload::Image {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

fn load_text(
    vfs: &dyn Vfs,
    path: &VPath,
    text_syntax: syntax::TextSyntax,
    cancel: &CancelToken,
) -> Result<PreviewPayload, PreviewError> {
    let Some((content, truncated)) = read_text(vfs, path, cancel)? else {
        return Ok(PreviewPayload::Unsupported);
    };
    let highlights =
        syntax::highlight(&content, text_syntax, cancel).map_err(|_| PreviewError::Cancelled)?;
    Ok(PreviewPayload::Text {
        content,
        language: text_syntax.language,
        truncated,
        highlights,
    })
}

fn read_text(
    vfs: &dyn Vfs,
    path: &VPath,
    cancel: &CancelToken,
) -> Result<Option<(String, bool)>, PreviewError> {
    let mut reader = vfs.open_read(path)?;
    let mut bytes = Vec::with_capacity(MAX_TEXT_BYTES.min(64 * 1_024));
    reader
        .by_ref()
        .take((MAX_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    let truncated = bytes.len() > MAX_TEXT_BYTES;
    bytes.truncate(MAX_TEXT_BYTES);
    if bytes.iter().take(8_192).any(|byte| *byte == 0) {
        return Ok(None);
    }
    let content = String::from_utf8_lossy(&bytes).into_owned();
    Ok(Some((content, truncated)))
}

fn is_image(extension: Option<&OsStr>) -> bool {
    matches!(
        extension
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "bmp" | "gif" | "ico" | "jpeg" | "jpg" | "png" | "pnm" | "tga" | "tif" | "tiff" | "webp"
    )
}

fn is_pdf(extension: Option<&OsStr>) -> bool {
    extension
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn is_media(extension: Option<&OsStr>) -> bool {
    matches!(
        extension
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "avi"
            | "flac"
            | "m4a"
            | "m4v"
            | "mkv"
            | "mov"
            | "mp3"
            | "mp4"
            | "mpeg"
            | "mpg"
            | "ogg"
            | "ogv"
            | "opus"
            | "wav"
            | "webm"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    use dualpane_core::VPath;
    use dualpane_vfs::{LocalFs, Vfs};

    use super::{
        CancelToken, PreviewPayload, ThumbnailRequest, ThumbnailScheduler, load_preview,
        load_thumbnail, supports_thumbnail,
    };

    #[test]
    fn loads_a_bounded_text_preview() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("sample.rs");
        fs::write(&path, "fn main() {}\n").expect("write sample");

        let preview =
            load_preview(&LocalFs, VPath::from(path), &CancelToken::new()).expect("load preview");
        let PreviewPayload::Text {
            content,
            language,
            truncated,
            highlights,
        } = preview.payload
        else {
            panic!("expected text preview");
        };
        assert_eq!(content, "fn main() {}\n");
        assert_eq!(language, "Rust");
        assert!(!truncated);
        assert!(!highlights.is_empty());
    }

    #[test]
    fn expanded_languages_and_named_files_reach_the_text_preview() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for (filename, source, expected_language) in [
            ("index.php", "<?php echo \"héllo\";\n", "PHP"),
            (
                "module.mts",
                "export const answer: number = 42;\n",
                "TypeScript",
            ),
            (
                "view.tsx",
                "const view = <button title=\"Hello\" />;\n",
                "TypeScript (TSX)",
            ),
            (
                "view.jsx",
                "const view = <button title=\"Hello\" />;\n",
                "JavaScript (JSX)",
            ),
            ("Cargo.lock", "version = 4\n", "TOML"),
            ("Dockerfile", "FROM alpine:3\n", "Dockerfile"),
            (
                "Makefile",
                "# build targets\nall:\n\techo hello\n",
                "Makefile",
            ),
            (
                "CMakeLists.txt",
                "# build configuration\nproject(commander)\n",
                "CMake",
            ),
            (
                ".env.local",
                "# local configuration\nNAME=commander\n",
                "DotENV",
            ),
        ] {
            let path = directory.path().join(filename);
            fs::write(&path, source).expect("write source");
            let preview =
                load_preview(&LocalFs, VPath::from(path), &CancelToken::new()).expect(filename);
            let PreviewPayload::Text {
                content,
                language,
                highlights,
                truncated,
            } = preview.payload
            else {
                panic!("expected text preview for {filename}");
            };
            assert_eq!(content, source, "{filename}");
            assert_eq!(language, expected_language, "{filename}");
            assert!(!highlights.is_empty(), "{filename}");
            assert!(!truncated, "{filename}");
        }
    }

    #[test]
    fn expanded_detection_keeps_binary_files_out_and_logs_plain() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for filename in ["binary.php", "binary.tsx", "binary.bin"] {
            let path = directory.path().join(filename);
            fs::write(&path, b"binary\0data").expect("write binary");
            let preview =
                load_preview(&LocalFs, VPath::from(path), &CancelToken::new()).expect(filename);
            assert!(
                matches!(preview.payload, PreviewPayload::Unsupported),
                "{filename}"
            );
        }
        let path = directory.path().join("app.log");
        let source = "<?php const message = 'plain log';";
        fs::write(&path, source).expect("write log");
        let preview =
            load_preview(&LocalFs, VPath::from(path), &CancelToken::new()).expect("preview log");
        let PreviewPayload::Text {
            content,
            highlights,
            ..
        } = preview.payload
        else {
            panic!("expected plain log preview");
        };
        assert_eq!(content, source);
        assert!(highlights.is_empty());
    }

    #[test]
    fn image_thumbnail_is_bounded_and_keeps_its_aspect_ratio() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("wide.png");
        image::RgbaImage::from_pixel(800, 400, image::Rgba([20, 80, 160, 255]))
            .save(&path)
            .expect("write image");

        let thumbnail = load_thumbnail(&LocalFs, &VPath::from(path), 128, &CancelToken::new())
            .expect("load thumbnail");

        assert_eq!((thumbnail.width, thumbnail.height), (128, 64));
        assert_eq!(thumbnail.rgba.len(), 128 * 64 * 4);
        assert!(thumbnail.fingerprint.size > 0);
    }

    #[test]
    fn scheduler_decodes_on_its_worker_pool() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("sample.PNG");
        image::RgbaImage::from_pixel(64, 32, image::Rgba([90, 30, 120, 255]))
            .save(&path)
            .expect("write image");
        let path = VPath::from(path);
        assert!(supports_thumbnail(&path));

        let vfs: Arc<dyn Vfs> = Arc::new(LocalFs);
        let scheduler =
            ThumbnailScheduler::with_capacity(vfs, 1, 4).expect("start thumbnail worker");
        let results = scheduler.results();
        scheduler
            .try_schedule(ThumbnailRequest {
                id: 17,
                path: path.clone(),
                max_edge: 48,
                cancel: CancelToken::new(),
            })
            .expect("schedule thumbnail");

        let response = results
            .recv_timeout(Duration::from_secs(5))
            .expect("thumbnail response");
        assert_eq!(response.id, 17);
        assert_eq!(response.path, path);
        assert!(response.result.is_ok());
    }
}
