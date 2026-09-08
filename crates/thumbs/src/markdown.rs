//! Bounded CommonMark parsing and local image decoding on the preview worker.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;

use dualpane_core::{CancelToken, VPath};
use dualpane_vfs::Vfs;
use image::imageops::FilterType;
use parser::{Event, Options, Parser, Tag};
use percent_encoding::percent_decode_str;
pub use pulldown_cmark as parser;

use crate::PreviewError;

const MAX_NODES: usize = 12_000;
const MAX_BLOCKS: usize = 512;
const MAX_DEPTH: usize = 48;
const MAX_IMAGES: usize = 8;
const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct MarkdownNode {
    pub event: Event<'static>,
    pub children: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct MarkdownImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// An arena avoids recursive ownership even for deeply nested input.
#[derive(Clone, Debug)]
pub struct MarkdownDocument {
    pub nodes: Vec<MarkdownNode>,
    pub images: HashMap<String, MarkdownImage>,
    pub truncated: bool,
}

pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkd" | "mkdn"
            )
        })
}

impl MarkdownDocument {
    pub(crate) fn parse(source: &str, cancel: &CancelToken) -> Result<Self, PreviewError> {
        cancel.check().map_err(|_| PreviewError::Cancelled)?;
        let mut document = Self {
            nodes: vec![MarkdownNode {
                event: Event::Start(Tag::Paragraph),
                children: Vec::new(),
            }],
            images: HashMap::new(),
            truncated: false,
        };
        let mut parents = vec![0];
        let mut blocks = 0;
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_GFM;
        for event in Parser::new_ext(source, options) {
            cancel.check().map_err(|_| PreviewError::Cancelled)?;
            if matches!(event, Event::End(_)) {
                parents.pop();
                continue;
            }
            let opens = matches!(event, Event::Start(_));
            if matches!(
                &event,
                Event::Start(
                    Tag::Paragraph
                        | Tag::Heading { .. }
                        | Tag::BlockQuote(_)
                        | Tag::CodeBlock(_)
                        | Tag::List(_)
                        | Tag::Item
                        | Tag::Table(_)
                        | Tag::TableHead
                        | Tag::TableRow
                        | Tag::TableCell
                        | Tag::FootnoteDefinition(_)
                        | Tag::HtmlBlock
                        | Tag::Image { .. }
                ) | Event::Rule
            ) {
                blocks += 1;
            }
            if document.nodes.len() >= MAX_NODES
                || blocks > MAX_BLOCKS
                || parents.len() >= MAX_DEPTH
            {
                document.truncated = true;
                break;
            }
            let index = document.nodes.len();
            document.nodes.push(MarkdownNode {
                event: event.into_static(),
                children: Vec::new(),
            });
            document.nodes[*parents.last().unwrap_or(&0)]
                .children
                .push(index);
            if opens {
                parents.push(index);
            }
        }
        Ok(document)
    }

    pub(crate) fn load_images(
        &mut self,
        vfs: &dyn Vfs,
        path: &VPath,
        cancel: &CancelToken,
    ) -> Result<(), PreviewError> {
        let mut attempted = 0;
        for node in &self.nodes {
            cancel.check().map_err(|_| PreviewError::Cancelled)?;
            let Event::Start(Tag::Image { dest_url, .. }) = &node.event else {
                continue;
            };
            if attempted >= MAX_IMAGES || self.images.contains_key(dest_url.as_ref()) {
                continue;
            }
            // Previewing a document never fetches remote images or data URLs.
            let Some(image_path) = local_image_path(path, dest_url) else {
                continue;
            };
            attempted += 1;
            if let Some(image) = decode_image(vfs, &image_path, cancel) {
                self.images.insert(dest_url.to_string(), image);
            }
        }
        cancel.check().map_err(|_| PreviewError::Cancelled)
    }

    /// Plain content for code blocks, accessible image descriptions, and anchors.
    pub fn plain_text(&self, index: usize) -> String {
        let mut text = String::new();
        let mut pending = vec![index];
        while let Some(index) = pending.pop() {
            let node = &self.nodes[index];
            match &node.event {
                Event::Text(value)
                | Event::Code(value)
                | Event::Html(value)
                | Event::InlineHtml(value) => text.push_str(value),
                Event::SoftBreak | Event::HardBreak => text.push('\n'),
                Event::TaskListMarker(checked) => {
                    text.push_str(if *checked { "☑ " } else { "☐ " })
                }
                _ => {}
            }
            pending.extend(node.children.iter().rev());
        }
        text
    }
}

fn local_image_path(document: &VPath, destination: &str) -> Option<VPath> {
    if destination.is_empty()
        || destination.contains(':')
        || destination.starts_with("//")
        || destination.starts_with('#')
    {
        return None;
    }
    let path = destination.split(['?', '#']).next()?;
    let decoded = percent_decode_str(path).decode_utf8().ok()?;
    if decoded.contains('\0') {
        return None;
    }
    Some(VPath::from(
        document.as_path().parent()?.join(decoded.as_ref()),
    ))
}

fn decode_image(vfs: &dyn Vfs, path: &VPath, cancel: &CancelToken) -> Option<MarkdownImage> {
    if vfs.stat(path, true).ok()?.size > MAX_IMAGE_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    vfs.open_read(path)
        .ok()?
        .take((MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_IMAGE_BYTES || cancel.check().is_err() {
        return None;
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    if cancel.check().is_err() {
        return None;
    }
    let image = if image.width() > 960 || image.height() > 720 {
        image.resize(960, 720, FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.into_rgba8();
    Some(MarkdownImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualpane_vfs::LocalFs;

    #[test]
    fn parses_commonmark_and_github_extensions_with_unicode() {
        let source = "# Héllo 🦀\n\n**bold** _italic_ ~~old~~ [guide][g]\n\n- [x] done\n- [ ] next\n\n> quoted\n\n```rust\nfn main() {}\n```\n\n| Name | Value |\n| :--- | ---: |\n| café | 42 |\n\n[g]: guide.md\n";
        let document = MarkdownDocument::parse(source, &CancelToken::new()).unwrap();
        assert!(!document.truncated);
        assert!(document.plain_text(0).contains("Héllo 🦀"));
        for predicate in [
            (|event: &Event<'_>| matches!(event, Event::Start(Tag::Heading { .. })))
                as fn(&Event<'_>) -> bool,
            |event| matches!(event, Event::Start(Tag::Table(_))),
            |event| matches!(event, Event::TaskListMarker(true)),
            |event| matches!(event, Event::Start(Tag::CodeBlock(_))),
            |event| matches!(event, Event::Start(Tag::Strikethrough)),
        ] {
            assert!(document.nodes.iter().any(|node| predicate(&node.event)));
        }
        assert!(document.nodes.iter().any(|node| matches!(&node.event, Event::Start(Tag::Link { dest_url, .. }) if dest_url.as_ref() == "guide.md")));
    }

    #[test]
    fn bounds_large_and_deep_documents_and_honors_cancellation() {
        let cancel = CancelToken::new();
        let large =
            MarkdownDocument::parse(&"paragraph\n\n".repeat(MAX_BLOCKS + 1), &cancel).unwrap();
        assert!(large.truncated);
        let deep = MarkdownDocument::parse(&format!("{}text", "> ".repeat(100)), &cancel).unwrap();
        assert!(deep.truncated);
        let images =
            MarkdownDocument::parse(&"![image](same.png) ".repeat(MAX_BLOCKS + 1), &cancel)
                .unwrap();
        assert!(images.truncated);
        let inline = MarkdownDocument::parse(&"*emphasis* ".repeat(MAX_NODES), &cancel).unwrap();
        assert!(inline.truncated);
        assert!(inline.nodes.len() <= MAX_NODES);
        cancel.cancel();
        assert!(matches!(
            MarkdownDocument::parse("# cancelled", &cancel),
            Err(PreviewError::Cancelled)
        ));
    }

    #[test]
    fn loads_relative_images_without_fetching_remote_resources() {
        let folder = tempfile::tempdir().unwrap();
        let path = VPath::from(folder.path().join("README.MD"));
        image::RgbaImage::new(24, 12)
            .save(folder.path().join("local image.png"))
            .unwrap();
        let mut document = MarkdownDocument::parse("![Local](local%20image.png)\n\n![Remote](https://example.com/a.png)\n\n![Missing](missing.png)", &CancelToken::new()).unwrap();
        document
            .load_images(&LocalFs, &path, &CancelToken::new())
            .unwrap();
        assert_eq!(document.images.len(), 1);
        assert_eq!(document.images["local%20image.png"].width, 24);
        assert!(is_markdown(path.as_path()));
        assert!(local_image_path(&path, "//example.com/a.png").is_none());
        assert!(local_image_path(&path, "data:image/png;base64,foo").is_none());
    }

    #[test]
    fn markdown_files_reach_the_formatted_preview_and_reject_binary_input() {
        let folder = tempfile::tempdir().unwrap();
        let path = VPath::from(folder.path().join("notes.MARKDOWN"));
        std::fs::write(path.as_path(), "# Notes\n\nA **formatted** document.").unwrap();
        let preview = crate::load_preview(&LocalFs, path.clone(), &CancelToken::new()).unwrap();
        assert!(matches!(
            preview.payload,
            crate::PreviewPayload::Markdown {
                truncated: false,
                ..
            }
        ));
        std::fs::write(path.as_path(), b"# Binary\0data").unwrap();
        let preview = crate::load_preview(&LocalFs, path, &CancelToken::new()).unwrap();
        assert!(matches!(
            preview.payload,
            crate::PreviewPayload::Unsupported
        ));
    }
}
