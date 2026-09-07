use std::collections::BTreeMap;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use dualpane_core::{CancelToken, EntryKind, Metadata, VPath};
use dualpane_vfs::Vfs;
use regex::Regex;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageOutputFormat {
    Png,
    Jpeg,
    WebP,
    Bmp,
}

impl ImageOutputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
            Self::Bmp => "bmp",
        }
    }

    const fn image_format(self) -> image::ImageFormat {
        match self {
            Self::Png => image::ImageFormat::Png,
            Self::Jpeg => image::ImageFormat::Jpeg,
            Self::WebP => image::ImageFormat::WebP,
            Self::Bmp => image::ImageFormat::Bmp,
        }
    }
}

/// Cheap menu/command eligibility using the converter's enabled decoders.
/// Actual image contents are validated by the decoder when conversion starts.
pub fn supports_image_conversion(path: &VPath, kind: EntryKind) -> bool {
    kind == EntryKind::File
        && path
            .as_path()
            .extension()
            .and_then(image::ImageFormat::from_extension)
            .is_some_and(|format| format.reading_enabled())
}

pub fn convert_image(
    vfs: &dyn Vfs,
    source: &VPath,
    destination: &VPath,
    format: ImageOutputFormat,
    cancel: &CancelToken,
) -> Result<(), String> {
    cancel
        .check()
        .map_err(|_| "Image conversion cancelled".to_owned())?;
    let reader = BufReader::new(vfs.open_read(source).map_err(|error| error.to_string())?);
    let mut reader = image::ImageReader::new(reader);
    // Formats such as TGA have no identifying header. Keep the extension as a
    // fallback while allowing recognized file contents to override it.
    if let Ok(format) = image::ImageFormat::from_path(source.as_path()) {
        reader.set_format(format);
    }
    let image = reader
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())?;
    cancel
        .check()
        .map_err(|_| "Image conversion cancelled".to_owned())?;
    let mut writer = vfs
        .create_write(destination, dualpane_core::SizeHint::Unknown)
        .map_err(|error| error.to_string())?;
    image
        .write_to(&mut writer, format.image_format())
        .map_err(|error| error.to_string())
}

const READ_BUFFER_SIZE: usize = 128 * 1_024;
const MAX_SEARCH_RESULTS: usize = 10_000;

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub query: String,
    pub search_content: bool,
    pub case_sensitive: bool,
    pub include_hidden: bool,
    pub max_depth: Option<usize>,
    pub max_content_bytes: u64,
    pub regex: bool,
    pub extension: Option<String>,
    pub kind: Option<EntryKind>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_after: Option<i64>,
    pub include_symlinks: bool,
    pub include_ignored: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            search_content: false,
            case_sensitive: false,
            include_hidden: false,
            max_depth: None,
            max_content_bytes: 8 * 1_024 * 1_024,
            regex: false,
            extension: None,
            kind: None,
            min_size: None,
            max_size: None,
            modified_after: None,
            include_symlinks: true,
            include_ignored: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub path: VPath,
    pub kind: EntryKind,
    pub size: u64,
    pub content_match: bool,
}

pub fn recursive_search(
    vfs: &dyn Vfs,
    root: &VPath,
    options: &SearchOptions,
    cancel: &CancelToken,
) -> Result<Vec<SearchHit>, String> {
    let regex = if options.regex {
        Some(
            regex::RegexBuilder::new(&options.query)
                .case_insensitive(!options.case_sensitive)
                .build()
                .map_err(|error| format!("Invalid regular expression: {error}"))?,
        )
    } else {
        None
    };
    let query = if options.case_sensitive {
        options.query.clone()
    } else {
        options.query.to_lowercase()
    };
    let mut hits = Vec::new();
    let mut pending = vec![(root.clone(), 0_usize)];
    while let Some((directory, depth)) = pending.pop() {
        cancel.check().map_err(|_| "Search cancelled".to_owned())?;
        if options.max_depth.is_some_and(|limit| depth > limit) {
            continue;
        }
        let entries = vfs
            .read_dir(&directory, cancel)
            .map_err(|error| format!("Could not search {directory}: {error}"))?;
        for entry in entries {
            cancel.check().map_err(|_| "Search cancelled".to_owned())?;
            let entry = entry.map_err(|error| error.to_string())?;
            if !options.include_hidden && entry.name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if !options.include_ignored
                && matches!(
                    entry.name().to_str(),
                    Some(".git" | ".hg" | ".svn" | "node_modules" | "target" | ".cache")
                )
            {
                continue;
            }
            if entry.kind() == EntryKind::Symlink && !options.include_symlinks {
                continue;
            }
            let path = directory.join_name(entry.name());
            let searchable_path = path.to_string();
            let name_matches = text_matches(
                &searchable_path,
                &query,
                options.case_sensitive,
                regex.as_ref(),
            );
            let metadata = vfs.stat(&path, false).ok();
            let content_match = options.search_content
                && entry.kind() == EntryKind::File
                && metadata
                    .as_ref()
                    .is_none_or(|metadata| metadata.size <= options.max_content_bytes)
                && file_contains(
                    vfs,
                    &path,
                    &query,
                    options.case_sensitive,
                    regex.as_ref(),
                    options.max_content_bytes,
                    cancel,
                )?;
            let extension_matches = options.extension.as_ref().is_none_or(|expected| {
                path.as_path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case(expected.trim_start_matches('.'))
                    })
            });
            let kind_matches = options.kind.is_none_or(|kind| entry.kind() == kind);
            let size_matches = metadata.as_ref().is_none_or(|metadata| {
                options
                    .min_size
                    .is_none_or(|minimum| metadata.size >= minimum)
                    && options
                        .max_size
                        .is_none_or(|maximum| metadata.size <= maximum)
            });
            let modified_matches = options.modified_after.is_none_or(|minimum| {
                metadata
                    .as_ref()
                    .and_then(|metadata| metadata.modified)
                    .is_some_and(|modified| modified.seconds >= minimum)
            });
            if (name_matches || content_match)
                && extension_matches
                && kind_matches
                && size_matches
                && modified_matches
            {
                hits.push(SearchHit {
                    path: path.clone(),
                    kind: entry.kind(),
                    size: metadata.as_ref().map_or(0, |metadata| metadata.size),
                    content_match,
                });
                if hits.len() >= MAX_SEARCH_RESULTS {
                    return Ok(hits);
                }
            }
            if entry.kind() == EntryKind::Directory {
                pending.push((path, depth + 1));
            }
        }
    }
    hits.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(hits)
}

fn file_contains(
    vfs: &dyn Vfs,
    path: &VPath,
    query: &str,
    case_sensitive: bool,
    regex: Option<&Regex>,
    max_bytes: u64,
    cancel: &CancelToken,
) -> Result<bool, String> {
    if query.is_empty() {
        return Ok(false);
    }
    let mut reader = vfs.open_read(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(max_bytes)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    cancel.check().map_err(|_| "Search cancelled".to_owned())?;
    if bytes.iter().take(8_192).any(|byte| *byte == 0) {
        return Ok(false);
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(text_matches(&text, query, case_sensitive, regex))
}

fn text_matches(text: &str, query: &str, case_sensitive: bool, regex: Option<&Regex>) -> bool {
    regex.map_or_else(
        || {
            if case_sensitive {
                text.contains(query)
            } else {
                text.to_lowercase().contains(query)
            }
        },
        |regex| regex.is_match(text),
    )
}

pub fn sha256(vfs: &dyn Vfs, path: &VPath, cancel: &CancelToken) -> Result<String, String> {
    let mut reader = vfs.open_read(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; READ_BUFFER_SIZE];
    loop {
        cancel
            .check()
            .map_err(|_| "Checksum cancelled".to_owned())?;
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompareStatus {
    LeftOnly,
    RightOnly,
    Different,
    Same,
}

#[derive(Clone, Debug)]
pub struct CompareEntry {
    pub relative_path: PathBuf,
    pub status: CompareStatus,
    pub left: Option<Metadata>,
    pub right: Option<Metadata>,
}

pub fn compare_directories(
    vfs: &dyn Vfs,
    left: &VPath,
    right: &VPath,
    cancel: &CancelToken,
) -> Result<Vec<CompareEntry>, String> {
    let left_entries = collect_tree(vfs, left, cancel)?;
    let right_entries = collect_tree(vfs, right, cancel)?;
    let mut paths: Vec<_> = left_entries
        .keys()
        .chain(right_entries.keys())
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths
        .into_iter()
        .map(|relative_path| {
            let left = left_entries.get(&relative_path).cloned();
            let right = right_entries.get(&relative_path).cloned();
            let status = match (&left, &right) {
                (Some(_), None) => CompareStatus::LeftOnly,
                (None, Some(_)) => CompareStatus::RightOnly,
                (Some(left), Some(right))
                    if left.kind != right.kind
                        || left.size != right.size
                        || left.modified != right.modified =>
                {
                    CompareStatus::Different
                }
                (Some(_), Some(_)) => CompareStatus::Same,
                (None, None) => unreachable!("path originated in one of the maps"),
            };
            CompareEntry {
                relative_path,
                status,
                left,
                right,
            }
        })
        .collect())
}

fn collect_tree(
    vfs: &dyn Vfs,
    root: &VPath,
    cancel: &CancelToken,
) -> Result<BTreeMap<PathBuf, Metadata>, String> {
    let mut result = BTreeMap::new();
    let mut pending = vec![(root.clone(), PathBuf::new())];
    while let Some((directory, relative)) = pending.pop() {
        cancel.check().map_err(|_| "Compare cancelled".to_owned())?;
        let entries = vfs
            .read_dir(&directory, cancel)
            .map_err(|error| format!("Could not compare {directory}: {error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = directory.join_name(entry.name());
            let child_relative = relative.join(Path::new(entry.name()));
            let metadata = vfs.stat(&path, false).map_err(|error| error.to_string())?;
            if metadata.kind == EntryKind::Directory {
                pending.push((path, child_relative.clone()));
            }
            result.insert(child_relative, metadata);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dualpane_core::{CancelToken, VPath};
    use dualpane_vfs::LocalFs;

    use super::{CompareStatus, SearchOptions, compare_directories, recursive_search, sha256};

    #[test]
    fn search_checksum_and_compare_use_the_vfs() {
        let fixture = tempfile::tempdir().expect("fixture");
        let left = fixture.path().join("left");
        let right = fixture.path().join("right");
        fs::create_dir_all(&left).expect("left");
        fs::create_dir_all(&right).expect("right");
        fs::write(left.join("needle.txt"), "find the needle").expect("left file");
        fs::write(right.join("needle.txt"), "different").expect("right file");
        let cancel = CancelToken::new();

        let hits = recursive_search(
            &LocalFs,
            &VPath::from(left.as_path()),
            &SearchOptions {
                query: "needle".to_owned(),
                search_content: true,
                ..SearchOptions::default()
            },
            &cancel,
        )
        .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            sha256(&LocalFs, &VPath::from(left.join("needle.txt")), &cancel).expect("checksum"),
            "9d6a242e1a2b40d9b3d72fbf34f34aac8a26e894868c78be4cd86610a72d7068"
        );
        let compared = compare_directories(
            &LocalFs,
            &VPath::from(left.as_path()),
            &VPath::from(right.as_path()),
            &cancel,
        )
        .expect("compare");
        assert_eq!(compared[0].status, CompareStatus::Different);
    }
}
