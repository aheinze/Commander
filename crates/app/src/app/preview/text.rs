//! GTK rendering of syntax spans prepared by the preview worker.

use dualpane_thumbs::{SyntaxKind, SyntaxSpan};
use relm4::adw::prelude::*;
use relm4::{adw, gtk};

const STYLES: [(SyntaxKind, &str, &str, &str); 9] = [
    (SyntaxKind::Comment, "syntax-comment", "#9a9996", "#5e5c64"),
    (SyntaxKind::String, "syntax-string", "#8ff0a4", "#18733b"),
    (SyntaxKind::Escape, "syntax-escape", "#f8e45c", "#8a5700"),
    (SyntaxKind::Keyword, "syntax-keyword", "#dc8add", "#813d9c"),
    (
        SyntaxKind::Operator,
        "syntax-operator",
        "#c0bfbc",
        "#5e5c64",
    ),
    (SyntaxKind::Type, "syntax-type", "#99c1f1", "#1c559e"),
    (
        SyntaxKind::Function,
        "syntax-function",
        "#62c4cf",
        "#086b79",
    ),
    (
        SyntaxKind::Constant,
        "syntax-constant",
        "#ffbe6f",
        "#a04200",
    ),
    (
        SyntaxKind::Attribute,
        "syntax-attribute",
        "#f8e45c",
        "#8a5700",
    ),
];

pub(super) fn new_view(class: &str) -> gtk::TextView {
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::None);
    view.add_css_class(class);
    let buffer = view.buffer();
    for (kind, name, _, _) in STYLES {
        let tag = gtk::TextTag::builder().name(name).build();
        if kind == SyntaxKind::Keyword {
            tag.set_weight(600);
        }
        buffer.tag_table().add(&tag);
    }
    let manager = adw::StyleManager::default();
    set_palette(&buffer, manager.is_dark());
    let weak_buffer = buffer.downgrade();
    manager.connect_dark_notify(move |manager| {
        if let Some(buffer) = weak_buffer.upgrade() {
            // Recolour existing tags without replacing text or losing selection.
            set_palette(&buffer, manager.is_dark());
        }
    });
    view
}

fn set_palette(buffer: &gtk::TextBuffer, dark: bool) {
    for (_, name, dark_color, light_color) in STYLES {
        if let Some(tag) = buffer.tag_table().lookup(name) {
            tag.set_foreground(Some(if dark { dark_color } else { light_color }));
        }
    }
}

pub(super) fn render(view: &gtk::TextView, content: &str, highlights: &[SyntaxSpan]) {
    let buffer = view.buffer();
    // Replacing the text clears the previous file's tag ranges. Reuse the fixed
    // tag table rather than accumulating one tag per token or per preview.
    buffer.set_text(content);
    let length = buffer.char_count();
    for (kind, name, _, _) in STYLES {
        let Some(tag) = buffer.tag_table().lookup(name) else {
            continue;
        };
        for span in highlights.iter().filter(|span| span.kind == kind) {
            let (Ok(start), Ok(end)) = (i32::try_from(span.start), i32::try_from(span.end)) else {
                continue;
            };
            if start < end && end <= length {
                buffer.apply_tag(
                    &tag,
                    &buffer.iter_at_offset(start),
                    &buffer.iter_at_offset(end),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a GTK display; run alone under Xvfb or a desktop session"]
    fn preview_tags_preserve_unicode_text_and_reset_when_switching_files() {
        adw::init().expect("initialize GTK");
        let view = new_view("preview-text");
        let content = "🦀 fn main() {}";
        let spans = [SyntaxSpan {
            start: 2,
            end: 4,
            kind: SyntaxKind::Keyword,
        }];
        render(&view, content, &spans);
        let buffer = view.buffer();
        let keyword = buffer.tag_table().lookup("syntax-keyword").unwrap();
        assert_eq!(
            buffer.text(&buffer.start_iter(), &buffer.end_iter(), false),
            content
        );
        assert!(buffer.iter_at_offset(2).has_tag(&keyword));
        assert!(!buffer.iter_at_offset(0).has_tag(&keyword));
        buffer.select_range(&buffer.iter_at_offset(2), &buffer.iter_at_offset(4));
        let manager = adw::StyleManager::default();
        let original_scheme = manager.color_scheme();
        manager.set_color_scheme(adw::ColorScheme::ForceDark);
        let dark = keyword.foreground_rgba();
        manager.set_color_scheme(adw::ColorScheme::ForceLight);
        assert_ne!(keyword.foreground_rgba(), dark);
        assert!(buffer.has_selection());
        manager.set_color_scheme(original_scheme);
        render(&view, "plain text", &[]);
        assert!(!buffer.iter_at_offset(2).has_tag(&keyword));
        assert_eq!(buffer.tag_table().size(), STYLES.len() as i32);
    }
}
