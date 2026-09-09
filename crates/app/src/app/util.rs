//! Small formatting and path helpers shared across the application modules.

use super::*;

pub(super) fn overridden_session(
    saved: &PaneSession,
    override_path: Option<&VPath>,
) -> PaneSession {
    override_path.map_or_else(
        || saved.clone(),
        |path| PaneSession {
            tabs: vec![path.to_string()],
            active_tab: 0,
            view_mode: saved.view_mode,
            sort_key: saved.sort_key,
            sort_descending: saved.sort_descending,
            show_hidden: saved.show_hidden,
            ..saved.clone()
        },
    )
}

pub(super) fn home_path() -> VPath {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map_or_else(|| VPath::from("/"), VPath::from)
}

pub(super) fn local_trash_path() -> VPath {
    let base = std::env::var_os("XDG_DATA_HOME").map_or_else(
        || {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("/"))
                .join(".local/share")
        },
        std::path::PathBuf::from,
    );
    VPath::from(base.join("Trash/files"))
}

pub(super) fn display_name(name: &OsStr) -> String {
    name.to_str().map_or_else(
        || format!("{}  [invalid UTF-8]", name.to_string_lossy()),
        str::to_owned,
    )
}

pub(super) fn estimated_grid_columns(width: i32) -> u32 {
    const GRID_HORIZONTAL_PADDING: i32 = 36;
    // The card's label can be 18 monospace characters wide, so its allocated
    // footprint is wider than the CSS minimum even for short filenames.
    const CARD_EXTENT: i32 = 152;
    u32::try_from((width - GRID_HORIZONTAL_PADDING).max(0) / CARD_EXTENT)
        .unwrap_or_default()
        .clamp(2, 12)
}

pub(super) fn miller_scroll_target(
    previous: f64,
    lower: f64,
    upper: f64,
    page_size: f64,
    reveal_new_column: bool,
) -> f64 {
    let maximum = (upper - page_size).max(lower);
    if reveal_new_column {
        maximum
    } else {
        previous.clamp(lower, maximum)
    }
}

pub(super) fn format_size(size: u64, kind: EntryKind) -> String {
    if kind == EntryKind::Directory {
        return "—".to_owned();
    }
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1_024.0 && unit + 1 < UNITS.len() {
        value /= 1_024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(super) fn valid_file_name(name: &str) -> bool {
    let mut components = std::path::Path::new(name).components();
    !name.is_empty()
        && matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

pub(super) fn is_archive_path(path: &VPath) -> bool {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.ends_with(".zip")
        || name.ends_with(".7z")
        || name.ends_with(".tar")
        || name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
}

pub(super) fn launch_custom_tool(
    tool: &CustomToolSession,
    paths: &[VPath],
) -> Result<String, String> {
    let tokens = shlex::split(&tool.command)
        .ok_or_else(|| format!("{} has invalid command quoting", tool.name))?;
    if tokens.is_empty() {
        return Err(format!("{} has an empty command", tool.name));
    }
    let launch = |path: &VPath, include_all: bool| -> Result<(), String> {
        let path_text = path.to_string();
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let parent = path
            .parent()
            .map_or_else(String::new, |path| path.to_string());
        let mut expanded = Vec::new();
        for token in &tokens {
            if token == "%paths%" && include_all {
                expanded.extend(paths.iter().map(ToString::to_string));
            } else {
                expanded.push(
                    token
                        .replace("%path%", &path_text)
                        .replace("%name%", &name)
                        .replace("%parent%", &parent)
                        .replace("%paths%", &path_text),
                );
            }
        }
        let (program, arguments) = expanded
            .split_first()
            .ok_or_else(|| format!("{} has an empty command", tool.name))?;
        std::process::Command::new(program)
            .args(arguments)
            .spawn()
            .map(drop)
            .map_err(|error| format!("Could not launch {}: {error}", tool.name))
    };
    if tokens.iter().any(|token| token == "%paths%") {
        let first = paths
            .first()
            .ok_or_else(|| "No selected paths".to_owned())?;
        launch(first, true)?;
    } else {
        for path in paths {
            launch(path, false)?;
        }
    }
    Ok(format!(
        "Launched {} for {} item(s)",
        tool.name,
        paths.len()
    ))
}

pub(super) fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
    let pattern = pattern.as_bytes();
    let candidate = candidate.as_bytes();
    let (mut pattern_index, mut candidate_index) = (0, 0);
    let (mut star, mut star_candidate) = (None, 0);
    while candidate_index < candidate.len() {
        if pattern
            .get(pattern_index)
            .is_some_and(|byte| *byte == b'?' || Some(byte) == candidate.get(candidate_index))
        {
            pattern_index += 1;
            candidate_index += 1;
        } else if pattern.get(pattern_index) == Some(&b'*') {
            star = Some(pattern_index);
            pattern_index += 1;
            star_candidate = candidate_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            star_candidate += 1;
            candidate_index = star_candidate;
        } else {
            return false;
        }
    }
    pattern[pattern_index..].iter().all(|byte| *byte == b'*')
}

pub(super) fn split_extension(name: &str, keep: bool) -> (String, String) {
    if keep
        && !name.starts_with('.')
        && let Some((stem, extension)) = name.rsplit_once('.')
    {
        return (stem.to_owned(), format!(".{extension}"));
    }
    (name.to_owned(), String::new())
}

pub(super) fn replace_text(
    source: &str,
    find: &str,
    replacement: &str,
    match_case: bool,
) -> String {
    if find.is_empty() {
        return source.to_owned();
    }
    if match_case {
        return source.replace(find, replacement);
    }
    if !source.is_ascii() || !find.is_ascii() {
        return source.replace(find, replacement);
    }
    let mut result = String::new();
    let mut remainder = source;
    let needle = find.to_lowercase();
    loop {
        let lowered = remainder.to_lowercase();
        let Some(byte) = lowered.find(&needle) else {
            result.push_str(remainder);
            break;
        };
        result.push_str(&remainder[..byte]);
        result.push_str(replacement);
        remainder = &remainder[byte + find.len()..];
    }
    result
}

pub(super) fn title_case(source: &str) -> String {
    source
        .split_inclusive(|character: char| !character.is_alphanumeric())
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(characters.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect()
}

pub(super) fn format_timestamp(seconds: i64) -> String {
    glib::DateTime::from_unix_local(seconds)
        .and_then(|value| value.format("%d %b %Y"))
        .map_or_else(|_| seconds.to_string(), |value| value.to_string())
}

pub(super) fn format_mode(mode: u32) -> String {
    let mut symbolic = String::with_capacity(9);
    for (bit, character) in [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ] {
        symbolic.push(if mode & bit == bit { character } else { '-' });
    }
    format!("{symbolic} · {:04o}", mode & 0o7777)
}

pub(super) const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

pub(super) fn permission_triplet(mode: u32, shift: u32) -> String {
    let value = (mode >> shift) & 0o7;
    format!(
        "{}  {}  {}",
        if value & 0o4 != 0 { "R" } else { "·" },
        if value & 0o2 != 0 { "W" } else { "·" },
        if value & 0o1 != 0 { "X" } else { "·" }
    )
}

pub(super) fn preview_type_label(preview: &Preview) -> String {
    match &preview.payload {
        PreviewPayload::Directory => "Folder".to_owned(),
        PreviewPayload::Table(document) => format!("{} spreadsheet", document.format),
        PreviewPayload::Markdown { .. } => "Markdown document".to_owned(),
        PreviewPayload::Image { .. } => "Image".to_owned(),
        PreviewPayload::Pdf(document) => {
            let page_count = document.pages.len();
            format!("PDF document · {page_count} page{}", plural(page_count))
        }
        PreviewPayload::Text { language, .. } => format!("{language} text"),
        PreviewPayload::Media { kind, .. } => (*kind).to_owned(),
        PreviewPayload::Unsupported => entry_kind_label(preview.metadata.kind).to_owned(),
    }
}

pub(super) const fn entry_kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Directory => "Folder",
        EntryKind::File => "File",
        EntryKind::Symlink => "Symbolic link",
        EntryKind::Fifo => "Named pipe",
        EntryKind::Socket => "Socket",
        EntryKind::CharacterDevice => "Character device",
        EntryKind::BlockDevice => "Block device",
        EntryKind::Unknown => "Item",
    }
}

pub(super) const fn job_kind_label(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Copy => "Copy",
        JobKind::Move => "Move",
        JobKind::Trash => "Move to Trash",
        JobKind::DeletePermanent => "Delete",
    }
}

pub(super) fn operation_log_label(message: &str) -> &str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("permission") {
        "Permissions"
    } else if lower.contains("archive") {
        "Archive"
    } else if lower.contains("pdf") {
        "PDF tools"
    } else if lower.contains("image") {
        "Image tools"
    } else if lower.contains("connected") || lower.contains("server") {
        "Remote storage"
    } else if lower.contains("job") || lower.contains("copy") || lower.contains("move") {
        "File operation"
    } else {
        "Activity"
    }
}

pub(super) fn relative_log_time(timestamp: SystemTime) -> String {
    let seconds = timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    i64::try_from(seconds)
        .ok()
        .and_then(|seconds| glib::DateTime::from_unix_local(seconds).ok())
        .and_then(|datetime| datetime.format("%H:%M:%S").ok())
        .map_or_else(|| "—".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(format_size(1_536, EntryKind::File), "1.5 KiB");
        assert_eq!(format_size(10, EntryKind::Directory), "—");
    }

    #[test]
    fn wildcard_globs_match_stars_and_question_marks() {
        assert!(wildcard_matches("*.rs", "selection.rs"));
        assert!(wildcard_matches("file-??", "file-42"));
        assert!(!wildcard_matches("*.rs", "selection.toml"));
    }

    #[test]
    fn file_names_cannot_escape_the_active_directory() {
        assert!(valid_file_name("report.txt"));
        assert!(!valid_file_name("../report.txt"));
        assert!(!valid_file_name("nested/report.txt"));
        assert!(!valid_file_name("."));
    }

    #[test]
    fn grid_keyboard_stride_tracks_the_visible_columns() {
        assert_eq!(estimated_grid_columns(200), 2);
        assert_eq!(estimated_grid_columns(656), 4);
        assert_eq!(estimated_grid_columns(4_000), 12);
    }

    #[test]
    fn miller_scroll_stays_put_until_a_new_column_opens() {
        assert_eq!(
            miller_scroll_target(280.0, 0.0, 1_000.0, 500.0, false),
            280.0
        );
        assert_eq!(
            miller_scroll_target(280.0, 0.0, 1_000.0, 500.0, true),
            500.0
        );
        assert_eq!(miller_scroll_target(480.0, 0.0, 700.0, 500.0, false), 200.0);
    }

    #[test]
    fn inspector_permissions_are_easy_to_scan() {
        assert_eq!(permission_triplet(0o754, 6), "R  W  X");
        assert_eq!(permission_triplet(0o754, 3), "R  ·  X");
        assert_eq!(permission_triplet(0o754, 0), "R  ·  ·");
    }

    #[test]
    fn inspector_log_labels_group_related_events() {
        assert_eq!(operation_log_label("Copy job 2 finished"), "File operation");
        assert_eq!(operation_log_label("Updated permissions"), "Permissions");
        assert_eq!(operation_log_label("Created PDF output"), "PDF tools");
    }
}
