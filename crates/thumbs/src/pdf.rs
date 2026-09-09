//! A parsed PDF stays on one worker; only geometry and bounded raster requests cross threads.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{
    Array, Dict, Name, Object, ObjectIdentifier, String as PdfString,
};

use crate::{CancelToken, PreviewError};

pub const MIN_SCALE: f32 = 0.05;
pub const MAX_SCALE: f32 = 2.4;
const MAX_PIXELS: f32 = 4_000_000.0;
const MAX_EDGE: f32 = 4096.0;
const MAX_PAGES: usize = 10_000;
const MAX_BOOKMARKS: usize = 2_000;
const MAX_BATCH: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub title: String,
    pub page: Option<usize>,
    pub depth: usize,
}

#[derive(Clone, Debug)]
pub struct PdfDocument {
    pub pages: Arc<[PageSize]>,
    pub bookmarks: Arc<[Bookmark]>,
    requests: Sender<Request>,
}

#[derive(Debug)]
struct Request {
    pages: Vec<(usize, f32)>,
    cancel: CancelToken,
    reply: Sender<PageResult>,
}

#[derive(Debug)]
pub struct Raster {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub struct PageResult {
    pub index: usize,
    pub result: Result<Raster, String>,
}

pub type PageResults = Receiver<PageResult>;

#[derive(Debug)]
pub enum RequestError {
    Busy,
    Stopped,
}

#[cfg(test)]
mod tests;

impl PdfDocument {
    pub fn load(bytes: Vec<u8>, cancel: &CancelToken) -> Result<Self, PreviewError> {
        cancel.check().map_err(|_| PreviewError::Cancelled)?;
        let (requests, queue) = bounded::<Request>(4);
        let (ready, metadata) = bounded(1);
        let loading_cancel = cancel.clone();
        std::thread::Builder::new()
            .name("commander-pdf".to_owned())
            .spawn(move || {
                let parsed = Pdf::new(bytes).map_err(|error| format!("{error:?}"));
                let pdf = match parsed {
                    Ok(pdf) => pdf,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if loading_cancel.check().is_err() {
                    return;
                }
                let sizes = page_sizes(&pdf);
                let sizes = match sizes {
                    Ok(sizes) => sizes,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                let bookmarks = bookmarks(&pdf);
                if ready.send(Ok((sizes.clone(), bookmarks))).is_err() {
                    return;
                }
                // This cache borrows the document and must stay on its worker.
                let mut cache = hayro::RenderCache::new();
                let mut rendered = 0;
                for request in queue {
                    for (index, scale) in request.pages {
                        if request.cancel.check().is_err() {
                            break;
                        }
                        // Release accumulated decoded fonts/images periodically.
                        if rendered == 16 {
                            cache = hayro::RenderCache::new();
                            rendered = 0;
                        }
                        let result = render(&pdf, &cache, &sizes, index, scale);
                        rendered += 1;
                        if request.cancel.check().is_err()
                            || request
                                .reply
                                .try_send(PageResult { index, result })
                                .is_err()
                        {
                            break;
                        }
                    }
                }
            })?;
        loop {
            cancel.check().map_err(|_| PreviewError::Cancelled)?;
            match metadata.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok((pages, bookmarks))) => {
                    return Ok(Self {
                        pages: pages.into(),
                        bookmarks: bookmarks.into(),
                        requests,
                    });
                }
                Ok(Err(error)) => return Err(PreviewError::Pdf(error)),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return Err(PreviewError::Pdf("PDF renderer stopped".to_owned()));
                }
            }
        }
    }

    /// Non-blocking submission. A full queue can be retried on the next UI frame.
    /// Each viewer owns its reply channel, so Quick Look cannot consume inspector results.
    pub fn request(
        &self,
        pages: Vec<(usize, f32)>,
        cancel: CancelToken,
    ) -> Result<PageResults, RequestError> {
        let (reply, receiver) = bounded(MAX_BATCH);
        let request = Request {
            pages: pages.into_iter().take(MAX_BATCH).collect(),
            cancel,
            reply,
        };
        self.requests
            .try_send(request)
            .map(|()| receiver)
            .map_err(|error| {
                if error.is_full() {
                    RequestError::Busy
                } else {
                    RequestError::Stopped
                }
            })
    }
}

fn page_sizes(pdf: &Pdf) -> Result<Vec<PageSize>, String> {
    let count = pdf.pages().len();
    if count == 0 {
        return Err("PDF has no pages".to_owned());
    }
    if count > MAX_PAGES {
        return Err(format!("PDF preview supports up to {MAX_PAGES} pages"));
    }
    pdf.pages()
        .iter()
        .map(|page| {
            let (width, height) = page.render_dimensions();
            if !width.is_finite()
                || !height.is_finite()
                || !(1.0..=14400.0).contains(&width)
                || !(1.0..=14400.0).contains(&height)
            {
                return Err("PDF page dimensions exceed preview limits".to_owned());
            }
            Ok(PageSize { width, height })
        })
        .collect()
}

fn raster_scale(size: PageSize, scale: f32) -> f32 {
    let scale = if scale.is_finite() { scale } else { 1.0 };
    scale
        .clamp(MIN_SCALE, MAX_SCALE * 2.0)
        .min(MAX_EDGE / size.width.max(size.height))
        .min((MAX_PIXELS / (size.width * size.height)).sqrt())
}

fn render<'a>(
    pdf: &'a Pdf,
    cache: &hayro::RenderCache<'a>,
    sizes: &[PageSize],
    index: usize,
    scale: f32,
) -> Result<Raster, String> {
    let page = pdf.pages().get(index).ok_or("PDF page does not exist")?;
    let scale = raster_scale(sizes[index], scale);
    let settings = hayro::RenderSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        ..Default::default()
    };
    let pixmap = hayro::render(page, cache, &InterpreterSettings::default(), &settings);
    Ok(Raster {
        width: u32::from(pixmap.width()),
        height: u32::from(pixmap.height()),
        rgba: pixmap
            .take_unpremultiplied()
            .into_iter()
            .flat_map(|pixel| [pixel.r, pixel.g, pixel.b, pixel.a])
            .collect(),
    })
}

fn bookmarks(pdf: &Pdf) -> Vec<Bookmark> {
    let xref = pdf.xref();
    let Some(catalog) = xref.get::<Dict<'_>>(xref.root_id()) else {
        return Vec::new();
    };
    let Some(first) = catalog
        .get::<Dict<'_>>(b"Outlines")
        .and_then(|outline| outline.get::<Dict<'_>>(b"First"))
    else {
        return Vec::new();
    };
    let pages: HashMap<_, _> = pdf
        .pages()
        .iter()
        .enumerate()
        .filter_map(|(index, page)| page.raw().obj_id().map(|id| (id, index)))
        .collect();
    let named = named_destinations(&catalog, &pages);
    let mut pending = vec![(first, 0)];
    let mut visited = HashSet::new();
    let mut output = Vec::new();
    // An iterative walk bounds malformed direct dictionaries as well as reference cycles.
    while let Some((item, depth)) = pending.pop() {
        if output.len() == MAX_BOOKMARKS {
            break;
        }
        if item.obj_id().is_some_and(|id| !visited.insert(id)) {
            continue;
        }
        let title = item
            .get::<PdfString<'_>>(b"Title")
            .map(|title| decode_title(title.as_bytes()))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Untitled bookmark".to_owned());
        let destination = item.get::<Object<'_>>(b"Dest").or_else(|| {
            let action = item.get::<Dict<'_>>(b"A")?;
            (action.get::<Name<'_>>(b"S").as_deref() == Some(b"GoTo".as_slice()))
                .then(|| action.get::<Object<'_>>(b"D"))
                .flatten()
        });
        let page = destination.and_then(|destination| match destination {
            Object::Name(name) => named.get(name.as_ref()).copied(),
            Object::String(name) => named.get(name.as_bytes()).copied(),
            other => explicit_destination(other, &pages),
        });
        output.push(Bookmark { title, page, depth });
        if let Some(next) = item.get::<Dict<'_>>(b"Next") {
            pending.push((next, depth));
        }
        if depth < 32
            && let Some(child) = item.get::<Dict<'_>>(b"First")
        {
            pending.push((child, depth + 1));
        }
    }
    output
}

fn explicit_destination(
    destination: Object<'_>,
    pages: &HashMap<ObjectIdentifier, usize>,
) -> Option<usize> {
    let array = match destination {
        Object::Array(array) => array,
        Object::Dict(dict) => dict.get::<Array<'_>>(b"D")?,
        _ => return None,
    };
    let reference = array.raw_iter().next()?.as_obj_ref()?;
    pages.get(&ObjectIdentifier::from(reference)).copied()
}

fn named_destinations(
    catalog: &Dict<'_>,
    pages: &HashMap<ObjectIdentifier, usize>,
) -> HashMap<Vec<u8>, usize> {
    let mut named = HashMap::new();
    if let Some(legacy) = catalog.get::<Dict<'_>>(b"Dests") {
        for name in legacy.keys().take(MAX_BOOKMARKS) {
            if name.len() <= 1024
                && let Some(page) = legacy
                    .get::<Object<'_>>(&name)
                    .and_then(|destination| explicit_destination(destination, pages))
            {
                named.insert(name.as_ref().to_vec(), page);
            }
        }
    }
    let mut pending: Vec<_> = catalog
        .get::<Dict<'_>>(b"Names")
        .and_then(|names| names.get::<Dict<'_>>(b"Dests"))
        .into_iter()
        .collect();
    let mut visited = HashSet::new();
    for _ in 0..MAX_BOOKMARKS {
        let Some(node) = pending.pop() else {
            break;
        };
        if node.obj_id().is_some_and(|id| !visited.insert(id)) {
            continue;
        }
        if let Some(entries) = node.get::<Array<'_>>(b"Names") {
            let mut entries = entries.iter::<Object<'_>>();
            for _ in 0..MAX_BOOKMARKS.saturating_sub(named.len()) {
                let (Some(Object::String(name)), Some(destination)) =
                    (entries.next(), entries.next())
                else {
                    break;
                };
                if name.len() <= 1024
                    && let Some(page) = explicit_destination(destination, pages)
                {
                    named.insert(name.as_bytes().to_vec(), page);
                }
            }
        }
        if let Some(children) = node.get::<Array<'_>>(b"Kids") {
            pending.extend(
                children
                    .iter::<Dict<'_>>()
                    .take(MAX_BOOKMARKS.saturating_sub(pending.len())),
            );
        }
    }
    named
}

fn decode_title(bytes: &[u8]) -> String {
    if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
        String::from_utf16_lossy(
            &bytes
                .as_chunks::<2>()
                .0
                .iter()
                .take(512)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )
    } else if let Some(bytes) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        String::from_utf8_lossy(bytes).chars().take(512).collect()
    } else {
        bytes
            .iter()
            .take(512)
            .map(|&byte| match byte {
                0x80 => '•',
                0x81 => '†',
                0x82 => '‡',
                0x83 => '…',
                0x84 => '—',
                0x85 => '–',
                0x86 => 'ƒ',
                0x87 => '⁄',
                0x88 => '‹',
                0x89 => '›',
                0x8a => '−',
                0x8b => '‰',
                0x8c => '„',
                0x8d => '“',
                0x8e => '”',
                0x8f => '‘',
                0x90 => '’',
                0x91 => '‚',
                0x92 => '™',
                0x93 => 'ﬁ',
                0x94 => 'ﬂ',
                0x95 => 'Ł',
                0x96 => 'Œ',
                0x97 => 'Š',
                0x98 => 'Ÿ',
                0x99 => 'Ž',
                0x9a => 'ı',
                0x9b => 'ł',
                0x9c => 'œ',
                0x9d => 'š',
                0x9e => 'ž',
                0xa0 => '€',
                _ => char::from(byte),
            })
            .collect()
    }
}
