use std::{cell::RefCell, collections::HashSet, rc::Rc};

use gtk::{glib, graphene, gsk};

use super::*;

struct RendererGuard(gsk::CairoRenderer);

impl Drop for RendererGuard {
    fn drop(&mut self) {
        self.0.unrealize();
    }
}

fn bundle() -> gio::Resource {
    gio::Resource::from_data(&glib::Bytes::from_static(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/icons.gresource"
    ))))
    .unwrap()
}

fn bundled_names() -> Vec<String> {
    bundle()
        .enumerate_children(
            &format!("{RESOURCE_PATH}/scalable/actions"),
            gio::ResourceLookupFlags::NONE,
        )
        .unwrap()
        .iter()
        .map(|filename| filename.strip_suffix(".svg").unwrap().to_owned())
        .collect()
}

#[test]
fn every_app_icon_reference_is_bundled_with_licenses() {
    let names: HashSet<_> = bundled_names().into_iter().collect();
    let pattern = regex::Regex::new(r#""(commander-[a-z0-9-]+-symbolic)""#).unwrap();
    let mut directories = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                directories.push(path);
            } else if path.extension() == Some(OsStr::new("rs")) {
                let source = std::fs::read_to_string(&path).unwrap();
                for captures in pattern.captures_iter(&source) {
                    assert!(
                        names.contains(&captures[1]),
                        "{} in {}",
                        &captures[1],
                        path.display()
                    );
                }
            }
        }
    }
    let license = bundle()
        .lookup_data(
            &format!("{RESOURCE_PATH}/LICENSE"),
            gio::ResourceLookupFlags::NONE,
        )
        .unwrap();
    let license = std::str::from_utf8(license.as_ref()).unwrap();
    assert!(license.contains("Lucide Icons and Contributors"));
    assert!(license.contains("Cole Bemis"));
}

#[test]
fn filenames_are_case_insensitive_and_entry_kind_takes_precedence() {
    for name in [
        "index.php",
        "index.PHTML",
        "component.tsx",
        "worker.MTS",
        "app.cts",
        "main.rs",
    ] {
        assert_eq!(
            for_entry(EntryKind::File, OsStr::new(name)).class,
            "icon-code",
            "{name}"
        );
    }
    for name in ["index.php", "image.JPG", "archive.tar.zst"] {
        assert_eq!(
            for_entry(EntryKind::Directory, OsStr::new(name)).class,
            "icon-folder"
        );
        assert_eq!(
            for_entry(EntryKind::Symlink, OsStr::new(name)).name,
            "commander-file-symlink-symbolic"
        );
    }
    assert_eq!(for_file(OsStr::new("image.JPG")).class, "icon-image");
    assert_eq!(
        for_file(OsStr::new("archive.tar.zst")).class,
        "icon-archive"
    );
    assert_eq!(
        for_file(OsStr::new(".env.production")).name,
        "commander-file-cog-symbolic"
    );
    assert_eq!(
        for_file(OsStr::new("Dockerfile.dev")).name,
        "commander-file-terminal-symbolic"
    );
    assert_eq!(for_file(OsStr::new("LICENSE")).class, "icon-document");
    assert_eq!(
        for_file(OsStr::new("unknown.extension")).name,
        "commander-file-symbolic"
    );
}

#[test]
#[cfg(unix)]
fn non_utf8_names_can_still_have_recognizable_extensions() {
    use std::os::unix::ffi::OsStrExt;
    assert_eq!(
        for_file(OsStr::from_bytes(b"photo-\xff.PNG")).class,
        "icon-image"
    );
    assert_eq!(
        for_file(OsStr::from_bytes(b"\xff")).name,
        "commander-file-symbolic"
    );
}

/// Real GTK SVG lookup, symbolic recoloring, scaling, CSS, and recycled rows.
/// Runs without presenting a window or touching the user's app configuration.
#[test]
#[ignore = "requires GTK; run alone with isolated XDG directories and --test-threads=1"]
fn gtk_bundled_icons_render_and_recycled_rows_keep_correct_colors() {
    assert!(std::env::var_os("COMMANDER_ISOLATED_TEST").is_some());
    relm4::adw::init().unwrap();
    let display = gdk::Display::default().unwrap();
    let theme = gtk::IconTheme::for_display(&display);
    let original_theme = theme.theme_name();
    install(&display);
    install(&display);
    assert_eq!(theme.theme_name(), original_theme);
    assert_eq!(
        theme
            .resource_path()
            .iter()
            .filter(|path| *path == RESOURCE_PATH)
            .count(),
        1
    );

    let guard = RendererGuard(gsk::CairoRenderer::new());
    let renderer = &guard.0;
    renderer.realize_for_display(&display).unwrap();
    for name in bundled_names() {
        for size in [16, 20, 24, 48] {
            for scale in [1, 2] {
                let icon = theme.lookup_icon(
                    &name,
                    &[],
                    size,
                    scale,
                    gtk::TextDirection::Ltr,
                    gtk::IconLookupFlags::FORCE_SYMBOLIC,
                );
                assert!(icon.is_symbolic(), "{name}");
                assert!(
                    icon.file()
                        .unwrap()
                        .uri()
                        .starts_with("resource:///org/example/Dualpane/icons/"),
                    "{name}"
                );
                for foreground in [gdk::RGBA::WHITE, gdk::RGBA::new(0.15, 0.3, 0.55, 1.0)] {
                    let pixels = size * scale;
                    let snapshot = gtk::Snapshot::new();
                    icon.snapshot_symbolic(
                        &snapshot,
                        f64::from(pixels),
                        f64::from(pixels),
                        &[foreground; 4],
                    );
                    let texture = renderer.render_texture(
                        snapshot.to_node().expect("icon produces a render node"),
                        Some(&graphene::Rect::new(0.0, 0.0, pixels as f32, pixels as f32)),
                    );
                    let mut rgba = vec![0; (pixels * pixels * 4) as usize];
                    texture.download(&mut rgba, (pixels * 4) as usize);
                    // Native Cairo pixels are premultiplied BGRA on this target.
                    // Opaque strokes must exist and cannot remain SVG source black.
                    assert!(
                        rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 128
                            && pixel[..3].iter().any(|channel| *channel > 20)),
                        "{name} at {size}×{scale}"
                    );
                }
            }
        }
    }

    let errors = Rc::new(RefCell::new(Vec::new()));
    let provider = gtk::CssProvider::new();
    let captured = errors.clone();
    provider
        .connect_parsing_error(move |_, _, error| captured.borrow_mut().push(error.to_string()));
    provider.load_from_string(include_str!("../../assets/style.css"));
    assert!(
        errors.borrow().is_empty(),
        "CSS errors: {:?}",
        errors.borrow()
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let image = gtk::Image::new();
    image.add_css_class("search-result-icon");
    let miller_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    miller_row.add_css_class("column-browser-row");
    miller_row.append(&image);
    for (kind, name) in [
        (EntryKind::Directory, "folder"),
        (EntryKind::File, "app.ts"),
        (EntryKind::File, "document.pdf"),
        (EntryKind::File, "README"),
        (EntryKind::File, "unknown"),
        (EntryKind::Symlink, "app.ts"),
    ] {
        set_file_icon(&image, kind, OsStr::new(name));
        let expected = for_entry(kind, OsStr::new(name));
        assert_eq!(image.icon_name().as_deref(), Some(expected.name));
        assert!(image.has_css_class("search-result-icon"));
        assert_eq!(
            FILE_CLASSES
                .iter()
                .filter(|class| image.has_css_class(class))
                .count(),
            1
        );
        assert!(image.has_css_class(expected.class));
        let reference = gtk::Image::new();
        set_file_icon(&reference, kind, OsStr::new(name));
        assert_eq!(
            image.color(),
            reference.color(),
            "Miller row must retain the {name} type color"
        );
    }

    if let Some(directory) = std::env::var_os("COMMANDER_ICON_PREVIEW_DIR") {
        render_gallery(&theme, renderer, Path::new(&directory), &display);
    }
    gtk::style_context_remove_provider_for_display(&display, &provider);
}

fn render_gallery(
    theme: &gtk::IconTheme,
    renderer: &gsk::CairoRenderer,
    directory: &Path,
    display: &gdk::Display,
) {
    std::fs::create_dir_all(directory).unwrap();
    let colors = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        display,
        &colors,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 10,
    );
    for (appearance, background, text, muted) in [
        ("dark", "#202227", "#e7e9ee", "#a0a8b4"),
        ("light", "#ffffff", "#252a33", "#737d8c"),
    ] {
        colors.load_from_string(&format!(
            "@define-color carelo_text {text}; @define-color carelo_icon {muted};"
        ));
        let foreground = gdk::RGBA::parse(text).unwrap();
        let snapshot = gtk::Snapshot::new();
        snapshot.append_color(
            &gdk::RGBA::parse(background).unwrap(),
            &graphene::Rect::new(0.0, 0.0, 800.0, 440.0),
        );
        let label = gtk::Label::new(None);
        let heading = label.create_pango_layout(Some(&format!("Commander / {appearance}")));
        heading.set_font_description(Some(&gtk::pango::FontDescription::from_string(
            "Sans Bold 19",
        )));
        snapshot.save();
        snapshot.translate(&graphene::Point::new(28.0, 20.0));
        snapshot.append_layout(&heading, &foreground);
        snapshot.restore();
        for (index, name) in [
            "chevron-left",
            "chevron-right",
            "arrow-up",
            "refresh-cw",
            "folder-plus",
            "copy",
            "trash",
            "search",
            "list",
            "layout-grid",
            "columns-3",
            "panel-left",
            "panel-right",
            "terminal",
            "settings-2",
        ]
        .iter()
        .enumerate()
        {
            let icon = theme.lookup_icon(
                &format!("commander-{name}-symbolic"),
                &[],
                20,
                1,
                gtk::TextDirection::Ltr,
                gtk::IconLookupFlags::FORCE_SYMBOLIC,
            );
            snapshot.save();
            snapshot.translate(&graphene::Point::new(30.0 + index as f32 * 50.0, 72.0));
            icon.snapshot_symbolic(&snapshot, 20.0, 20.0, &[foreground; 4]);
            snapshot.restore();
        }
        for (index, (kind, name)) in [
            (EntryKind::Directory, "Projects"),
            (EntryKind::File, "index.php"),
            (EntryKind::File, "App.tsx"),
            (EntryKind::File, "package.json"),
            (EntryKind::File, "photo.jpg"),
            (EntryKind::File, "music.flac"),
            (EntryKind::File, "video.mp4"),
            (EntryKind::File, "backup.tar.zst"),
            (EntryKind::File, "guide.pdf"),
            (EntryKind::File, "README.md"),
            (EntryKind::File, "budget.xlsx"),
            (EntryKind::File, "slides.pptx"),
            (EntryKind::File, "data.sqlite"),
            (EntryKind::File, ".env.local"),
            (EntryKind::Symlink, "linked-folder"),
        ]
        .into_iter()
        .enumerate()
        {
            let image = gtk::Image::new();
            set_file_icon(&image, kind, OsStr::new(name));
            let color = image.color();
            let icon = theme.lookup_icon(
                image.icon_name().as_deref().unwrap(),
                &[],
                40,
                1,
                gtk::TextDirection::Ltr,
                gtk::IconLookupFlags::FORCE_SYMBOLIC,
            );
            let x = 30.0 + (index % 5) as f32 * 154.0;
            let y = 132.0 + (index / 5) as f32 * 99.0;
            snapshot.save();
            snapshot.translate(&graphene::Point::new(x, y));
            icon.snapshot_symbolic(&snapshot, 40.0, 40.0, &[color; 4]);
            snapshot.translate(&graphene::Point::new(0.0, 51.0));
            let caption = label.create_pango_layout(Some(name));
            caption.set_font_description(Some(&gtk::pango::FontDescription::from_string(
                "Monospace 10",
            )));
            snapshot.append_layout(&caption, &foreground);
            snapshot.restore();
        }
        renderer
            .render_texture(snapshot.to_node().unwrap(), None)
            .save_to_png(directory.join(format!("icons-{appearance}.png")))
            .unwrap();
    }
    gtk::style_context_remove_provider_for_display(display, &colors);
}
