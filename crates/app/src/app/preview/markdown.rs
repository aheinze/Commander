//! Native Markdown widgets. Input is parsed and images are decoded by the worker.

use super::super::*;
use dualpane_thumbs::markdown::{
    MarkdownDocument,
    parser::{Alignment, Event, Tag},
};

#[cfg(test)]
mod tests;

type Anchors = Rc<RefCell<BTreeMap<String, glib::WeakRef<gtk::Widget>>>>;

pub(super) struct MarkdownView {
    pub root: gtk::ScrolledWindow,
    content: gtk::Box,
}

impl MarkdownView {
    pub fn new() -> Self {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.add_css_class("markdown-content");
        content.set_valign(gtk::Align::Start);
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&content)
            .build();
        root.set_overlay_scrolling(false);
        Self { root, content }
    }

    pub fn render(&self, document: &MarkdownDocument, path: &VPath, truncated: bool) {
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        let renderer = Renderer {
            document,
            base_uri: gio::File::for_path(path.as_path()).uri().to_string(),
            anchors: Rc::default(),
            scroll: self.root.downgrade(),
            textures: document
                .images
                .iter()
                .map(|(destination, image)| {
                    (
                        destination.clone(),
                        gdk::MemoryTexture::new(
                            image.width as i32,
                            image.height as i32,
                            gdk::MemoryFormat::R8g8b8a8,
                            &glib::Bytes::from_owned(image.rgba.clone()),
                            image.width as usize * 4,
                        ),
                    )
                })
                .collect(),
        };
        renderer.append_nodes(&self.content, &document.nodes[0].children);
        if self.content.first_child().is_none() {
            let empty = gtk::Label::new(Some("This Markdown file is empty."));
            empty.add_css_class("dim-label");
            self.content.append(&empty);
        }
        if truncated {
            notifications::info("Preview truncated. Open the file to read the complete document.");
        }
        self.root.vadjustment().set_value(0.0);
    }
}

struct Renderer<'a> {
    document: &'a MarkdownDocument,
    base_uri: String,
    anchors: Anchors,
    scroll: glib::WeakRef<gtk::ScrolledWindow>,
    textures: BTreeMap<String, gdk::MemoryTexture>,
}

impl Renderer<'_> {
    fn append_nodes(&self, parent: &gtk::Box, nodes: &[usize]) {
        let mut inline = Vec::new();
        for &index in nodes {
            if is_block(&self.document.nodes[index].event) {
                self.append_inline(parent, &inline);
                inline.clear();
                self.append_block(parent, index);
            } else {
                inline.push(index);
            }
        }
        self.append_inline(parent, &inline);
    }

    fn append_block(&self, parent: &gtk::Box, index: usize) {
        let node = &self.document.nodes[index];
        match &node.event {
            Event::Start(Tag::Heading { level, .. }) => {
                let label = self.label(&self.inline_markup(&node.children));
                label.add_css_class("markdown-heading");
                label.add_css_class(&format!("markdown-h{}", *level as u8));
                let slug = heading_slug(&self.document.plain_text(index));
                self.register_anchor(&slug, label.upcast_ref());
                parent.append(&label);
            }
            Event::Start(Tag::CodeBlock(_)) | Event::Html(_) => {
                let code = self.document.plain_text(index);
                let view = super::text::new_view("markdown-code");
                super::text::render(&view, code.trim_end_matches('\n'), &[]);
                let scroll = gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Automatic)
                    .vscrollbar_policy(gtk::PolicyType::Automatic)
                    .min_content_height((code.lines().count().clamp(1, 14) * 19 + 24) as i32)
                    .child(&view)
                    .build();
                scroll.add_css_class("markdown-code-scroll");
                parent.append(&scroll);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                let quote = gtk::Box::new(gtk::Orientation::Vertical, 8);
                quote.add_css_class("markdown-quote");
                self.append_nodes(&quote, &node.children);
                parent.append(&quote);
            }
            Event::Start(Tag::List(start)) => {
                let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
                list.add_css_class("markdown-list");
                for (position, &item) in node.children.iter().enumerate() {
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let children = &self.document.nodes[item].children;
                    let task = children.first().is_some_and(|&child| {
                        let node = &self.document.nodes[child];
                        matches!(node.event, Event::TaskListMarker(_))
                            || node.children.first().is_some_and(|&first| {
                                matches!(self.document.nodes[first].event, Event::TaskListMarker(_))
                            })
                    });
                    if !task {
                        let marker = start.map_or_else(
                            || "•".to_owned(),
                            |start| format!("{}.", start.saturating_add(position as u64)),
                        );
                        let marker = gtk::Label::new(Some(&marker));
                        marker.set_valign(gtk::Align::Start);
                        marker.add_css_class("markdown-list-marker");
                        row.append(&marker);
                    }
                    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
                    body.set_hexpand(true);
                    self.append_nodes(&body, children);
                    row.append(&body);
                    list.append(&row);
                }
                parent.append(&list);
            }
            Event::Start(Tag::Table(alignments)) => {
                let grid = gtk::Grid::new();
                grid.add_css_class("markdown-table");
                for (row_index, &row) in node.children.iter().enumerate() {
                    let row = &self.document.nodes[row];
                    for (column, &cell) in row.children.iter().enumerate() {
                        let label =
                            self.label(&self.inline_markup(&self.document.nodes[cell].children));
                        label.add_css_class("markdown-table-cell");
                        label.set_width_chars(10);
                        label.set_max_width_chars(24);
                        label.set_xalign(match alignments.get(column) {
                            Some(Alignment::Center) => 0.5,
                            Some(Alignment::Right) => 1.0,
                            _ => 0.0,
                        });
                        if matches!(row.event, Event::Start(Tag::TableHead)) {
                            label.add_css_class("markdown-table-heading");
                        }
                        grid.attach(&label, column as i32, row_index as i32, 1, 1);
                    }
                }
                let scroll = gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Automatic)
                    .vscrollbar_policy(gtk::PolicyType::Never)
                    .propagate_natural_height(true)
                    .child(&grid)
                    .build();
                parent.append(&scroll);
            }
            Event::Start(Tag::FootnoteDefinition(name)) => {
                let footnote = gtk::Box::new(gtk::Orientation::Vertical, 4);
                self.register_anchor(&format!("fn-{name}"), footnote.upcast_ref());
                footnote.append(&self.label(&format!("<b>[{}]</b>", escape(name))));
                self.append_nodes(&footnote, &node.children);
                parent.append(&footnote);
            }
            Event::Rule => parent.append(&gtk::Separator::new(gtk::Orientation::Horizontal)),
            _ => self.append_nodes(parent, &node.children),
        }
    }

    fn append_inline(&self, parent: &gtk::Box, nodes: &[usize]) {
        if nodes.is_empty() {
            return;
        }
        let markup = self.inline_markup(nodes);
        if !markup.trim().is_empty() {
            parent.append(&self.label(&markup));
        }
        // Images are separate native blocks so they can shrink to narrow inspectors.
        let mut pending = nodes.iter().rev().copied().collect::<Vec<_>>();
        while let Some(index) = pending.pop() {
            let node = &self.document.nodes[index];
            if let Event::Start(Tag::Image {
                dest_url, title, ..
            }) = &node.event
                && let Some(texture) = self.textures.get(dest_url.as_ref())
            {
                let picture = gtk::Picture::for_paintable(texture);
                picture.set_can_shrink(true);
                picture.set_content_fit(gtk::ContentFit::ScaleDown);
                picture.set_height_request(texture.height().min(320));
                let alt = self.document.plain_text(index);
                picture.update_property(&[gtk::accessible::Property::Label(&alt)]);
                if !title.is_empty() {
                    picture.set_tooltip_text(Some(title));
                }
                parent.append(&picture);
            } else {
                pending.extend(node.children.iter().rev());
            }
        }
    }

    fn inline_markup(&self, nodes: &[usize]) -> String {
        nodes.iter().map(|&index| self.node_markup(index)).collect()
    }

    fn node_markup(&self, index: usize) -> String {
        let node = &self.document.nodes[index];
        match &node.event {
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => escape(text),
            Event::Code(text) => format!("<tt>{}</tt>", escape(text)),
            Event::SoftBreak => " ".to_owned(),
            Event::HardBreak => "\n".to_owned(),
            Event::TaskListMarker(checked) => if *checked { "☑ " } else { "☐ " }.to_owned(),
            Event::FootnoteReference(name) => format!(
                "<a href=\"#fn-{}\"><sup>[{}]</sup></a>",
                escape(name),
                escape(name)
            ),
            Event::Start(Tag::Emphasis) => format!("<i>{}</i>", self.inline_markup(&node.children)),
            Event::Start(Tag::Strong) => format!("<b>{}</b>", self.inline_markup(&node.children)),
            Event::Start(Tag::Strikethrough) => {
                format!("<s>{}</s>", self.inline_markup(&node.children))
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                self.link_markup(dest_url, &self.inline_markup(&node.children))
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                if self.document.images.contains_key(dest_url.as_ref()) {
                    return String::new();
                }
                let alt = self.document.plain_text(index);
                escape(&format!(
                    "[Image: {}]",
                    if alt.is_empty() {
                        dest_url.as_ref()
                    } else {
                        &alt
                    }
                ))
            }
            _ => self.inline_markup(&node.children),
        }
    }

    fn link_markup(&self, destination: &str, content: &str) -> String {
        safe_link(&self.base_uri, destination).map_or_else(
            || content.to_owned(),
            |uri| format!("<a href=\"{}\">{content}</a>", escape(&uri)),
        )
    }

    fn label(&self, markup: &str) -> gtk::Label {
        let label = gtk::Label::new(None);
        label.set_markup(markup);
        label.set_selectable(true);
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_width_chars(1);
        label.set_xalign(0.0);
        label.set_hexpand(true);
        let anchors = Rc::clone(&self.anchors);
        let scroll = self.scroll.clone();
        label.connect_activate_link(move |_, uri| {
            let Some(fragment) = uri.strip_prefix('#') else {
                return glib::Propagation::Proceed;
            };
            if let Some(target) = anchors
                .borrow()
                .get(fragment)
                .and_then(glib::WeakRef::upgrade)
                && let Some(scroll) = scroll.upgrade()
                && let Some(bounds) = target.compute_bounds(&scroll)
            {
                let adjustment = scroll.vadjustment();
                adjustment.set_value((adjustment.value() + f64::from(bounds.y()) - 12.0).max(0.0));
            }
            glib::Propagation::Stop
        });
        label
    }

    fn register_anchor(&self, slug: &str, widget: &gtk::Widget) {
        let mut anchors = self.anchors.borrow_mut();
        let mut key = slug.to_owned();
        let mut duplicate = 1;
        while anchors.contains_key(&key) {
            key = format!("{slug}-{duplicate}");
            duplicate += 1;
        }
        anchors.insert(key, widget.downgrade());
    }
}

fn is_block(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Start(
            Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::CodeBlock(_)
                | Tag::BlockQuote(_)
                | Tag::List(_)
                | Tag::Table(_)
                | Tag::FootnoteDefinition(_)
                | Tag::HtmlBlock
        ) | Event::Rule
            | Event::Html(_)
    )
}

fn escape(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

fn heading_slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|character| {
            character.is_alphanumeric()
                || character.is_whitespace()
                || *character == '-'
                || *character == '_'
        })
        .map(|character| {
            if character.is_whitespace() {
                '-'
            } else {
                character
            }
        })
        .collect()
}

fn safe_link(base: &str, destination: &str) -> Option<String> {
    if let Some(fragment) = destination.strip_prefix('#') {
        return glib::Uri::unescape_string(fragment, None).map(|fragment| format!("#{fragment}"));
    }
    let uri = glib::Uri::resolve_relative(Some(base), destination, glib::UriFlags::NONE).ok()?;
    let scheme = glib::Uri::peek_scheme(&uri)?;
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto" | "file"
    )
    .then(|| uri.to_string())
}
