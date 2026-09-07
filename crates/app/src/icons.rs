//! Embedded symbolic SVGs and filename-only file classification. GTK owns the
//! scaled paintable cache; binding a file row performs no I/O or MIME probing.

use std::{ffi::OsStr, path::Path, sync::Once};

use dualpane_core::EntryKind;
use relm4::gtk::{self, gdk, gio, prelude::*};

const RESOURCE_PATH: &str = "/org/example/Dualpane/icons";
const FILE_CLASSES: &[&str] = &[
    "icon-folder",
    "icon-code",
    "icon-data",
    "icon-image",
    "icon-media",
    "icon-archive",
    "icon-document",
    "icon-pdf",
    "icon-sheet",
    "icon-muted",
];

pub(crate) fn install(display: &gdk::Display) {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        gio::resources_register_include!("icons.gresource")
            .expect("the compiled icon bundle must be valid");
    });
    let theme = gtk::IconTheme::for_display(display);
    if !theme
        .resource_path()
        .iter()
        .any(|path| path == RESOURCE_PATH)
    {
        theme.add_resource_path(RESOURCE_PATH);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIcon {
    name: &'static str,
    class: &'static str,
}

impl FileIcon {
    const fn new(name: &'static str, class: &'static str) -> Self {
        Self { name, class }
    }
}

/// Also clear the previous file category when GTK recycles a row or preview.
pub(crate) fn set_file_icon(image: &gtk::Image, kind: EntryKind, name: &OsStr) {
    let icon = for_entry(kind, name);
    if image.icon_name().as_deref() == Some(icon.name) && image.has_css_class(icon.class) {
        return;
    }
    image.set_icon_name(Some(icon.name));
    image.add_css_class("commander-file-icon");
    for class in FILE_CLASSES {
        if *class != icon.class {
            image.remove_css_class(class);
        }
    }
    image.add_css_class(icon.class);
}

fn for_entry(kind: EntryKind, name: &OsStr) -> FileIcon {
    match kind {
        EntryKind::Directory => FileIcon::new("commander-folder-symbolic", "icon-folder"),
        EntryKind::Symlink => FileIcon::new("commander-file-symlink-symbolic", "icon-muted"),
        EntryKind::Socket => FileIcon::new("commander-network-symbolic", "icon-muted"),
        EntryKind::CharacterDevice | EntryKind::BlockDevice => {
            FileIcon::new("commander-hard-drive-symbolic", "icon-muted")
        }
        EntryKind::Fifo | EntryKind::Unknown => {
            FileIcon::new("commander-file-symbolic", "icon-muted")
        }
        EntryKind::File => for_file(name),
    }
}

fn matches_any(value: &str, choices: &[&str]) -> bool {
    choices
        .iter()
        .any(|choice| value.eq_ignore_ascii_case(choice))
}

fn for_file(name: &OsStr) -> FileIcon {
    let filename = name.to_str().unwrap_or_default();
    let extension = Path::new(name)
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if matches_any(
        filename,
        &[
            "Dockerfile",
            "Containerfile",
            "Makefile",
            "GNUmakefile",
            "Justfile",
            "PKGBUILD",
        ],
    ) || filename
        .split_once('.')
        .is_some_and(|(prefix, _)| matches_any(prefix, &["Dockerfile", "Containerfile"]))
    {
        return FileIcon::new("commander-file-terminal-symbolic", "icon-code");
    }
    if matches_any(
        filename,
        &[
            ".env",
            ".gitignore",
            ".gitattributes",
            ".gitmodules",
            ".editorconfig",
            "Cargo.lock",
            "CMakeLists.txt",
        ],
    ) || filename
        .as_bytes()
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b".env."))
    {
        return FileIcon::new("commander-file-cog-symbolic", "icon-muted");
    }
    if matches_any(
        filename,
        &[
            ".bashrc",
            ".bash_profile",
            ".zshrc",
            ".zprofile",
            ".profile",
        ],
    ) || matches_any(
        extension,
        &["sh", "bash", "zsh", "fish", "ps1", "bat", "cmd"],
    ) {
        return FileIcon::new("commander-file-terminal-symbolic", "icon-code");
    }
    if matches_any(
        extension,
        &[
            "rs", "c", "h", "cc", "cpp", "cxx", "hpp", "m", "mm", "go", "py", "pyw", "rb", "php",
            "phtml", "php3", "php4", "php5", "php7", "php8", "phps", "js", "jsx", "mjs", "cjs",
            "ts", "tsx", "mts", "cts", "vue", "svelte", "java", "kt", "kts", "scala", "cs", "fs",
            "fsx", "swift", "dart", "lua", "pl", "pm", "r", "ex", "exs", "erl", "hrl", "hs", "clj",
            "cljs", "cljc", "edn", "zig", "nim", "nix", "asm", "s",
        ],
    ) {
        return FileIcon::new("commander-file-code-symbolic", "icon-code");
    }
    if matches_any(
        extension,
        &[
            "html", "htm", "xhtml", "css", "scss", "sass", "less", "json", "jsonc", "json5", "xml",
            "yaml", "yml", "toml", "graphql", "gql",
        ],
    ) {
        return FileIcon::new("commander-file-braces-symbolic", "icon-data");
    }
    if matches_any(
        extension,
        &[
            "png", "jpg", "jpeg", "gif", "webp", "avif", "heic", "heif", "bmp", "tif", "tiff",
            "svg", "svgz", "ico", "icns", "psd", "xcf", "ora", "raw", "dng",
        ],
    ) {
        return FileIcon::new("commander-file-image-symbolic", "icon-image");
    }
    if matches_any(
        extension,
        &[
            "mp3", "flac", "wav", "ogg", "oga", "opus", "m4a", "aac", "aiff", "aif", "wma", "mid",
            "midi",
        ],
    ) {
        return FileIcon::new("commander-file-music-symbolic", "icon-media");
    }
    if matches_any(
        extension,
        &[
            "mp4", "mkv", "webm", "mov", "avi", "m4v", "mpeg", "mpg", "ogv", "wmv", "m2ts",
        ],
    ) {
        return FileIcon::new("commander-file-video-camera-symbolic", "icon-media");
    }
    if matches_any(
        extension,
        &[
            "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz", "zst",
            "zstd", "lz", "lzma", "lz4", "deb", "rpm", "pkg", "apk", "jar", "war", "iso",
        ],
    ) {
        return FileIcon::new("commander-file-archive-symbolic", "icon-archive");
    }
    if extension.eq_ignore_ascii_case("pdf") {
        return FileIcon::new("commander-file-text-symbolic", "icon-pdf");
    }
    if matches_any(
        extension,
        &["csv", "tsv", "xls", "xlsx", "xlsm", "ods", "numbers"],
    ) {
        return FileIcon::new("commander-file-spreadsheet-symbolic", "icon-sheet");
    }
    if matches_any(extension, &["ppt", "pptx", "odp"]) {
        return FileIcon::new("commander-file-chart-pie-symbolic", "icon-archive");
    }
    if matches_any(
        extension,
        &[
            "txt", "md", "markdown", "rst", "adoc", "org", "log", "rtf", "doc", "docx", "odt",
            "pages", "epub", "tex",
        ],
    ) || filename.split('.').next().is_some_and(|stem| {
        matches_any(
            stem,
            &[
                "README",
                "LICENSE",
                "LICENCE",
                "COPYING",
                "CHANGELOG",
                "AUTHORS",
                "NOTICE",
                "TODO",
            ],
        )
    }) {
        return FileIcon::new("commander-file-text-symbolic", "icon-document");
    }
    if matches_any(extension, &["db", "sqlite", "sqlite3", "mdb", "sql"]) {
        return FileIcon::new("commander-database-symbolic", "icon-data");
    }
    if matches_any(extension, &["ttf", "otf", "woff", "woff2", "eot"]) {
        return FileIcon::new("commander-file-type-symbolic", "icon-document");
    }
    if matches_any(
        extension,
        &[
            "key", "pem", "crt", "cer", "p12", "pfx", "pub", "asc", "gpg",
        ],
    ) || matches_any(filename, &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"])
    {
        return FileIcon::new("commander-file-key-symbolic", "icon-archive");
    }
    if matches_any(
        extension,
        &[
            "ini",
            "conf",
            "cfg",
            "config",
            "properties",
            "desktop",
            "service",
            "socket",
            "timer",
            "lock",
        ],
    ) {
        return FileIcon::new("commander-file-cog-symbolic", "icon-muted");
    }
    if matches_any(
        extension,
        &[
            "appimage", "exe", "msi", "dll", "so", "dylib", "bin", "wasm",
        ],
    ) {
        return FileIcon::new("commander-app-window-symbolic", "icon-muted");
    }
    FileIcon::new("commander-file-symbolic", "icon-muted")
}

#[cfg(test)]
mod tests;
