//! Bounded recursive search. Recoverable filesystem errors retain useful results.

use std::io::Read;

use dualpane_core::{CancelToken, EntryKind, VPath};
use dualpane_vfs::Vfs;
use regex::Regex;

const MAX_SEARCH_RESULTS: usize = 10_000;
pub const MAX_CONTENT_BYTES: u64 = 64 * 1_024 * 1_024;
const READ_BUFFER_SIZE: usize = 128 * 1_024;

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
    /// Include link entries; directory links are never followed.
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchHit {
    pub path: VPath,
    pub kind: EntryKind,
    pub size: u64,
    pub content_match: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub skipped_entries: usize,
    pub first_error: Option<String>,
    pub limit_reached: bool,
}

impl SearchResults {
    fn skip(&mut self, error: impl ToString) {
        self.skipped_entries += 1;
        if self.first_error.is_none() {
            self.first_error = Some(error.to_string());
        }
    }

    pub fn summary(&self) -> String {
        let mut summary = match self.hits.len() {
            0 => "No matching files".to_owned(),
            1 => "1 result".to_owned(),
            count => format!("{count} results"),
        };
        if self.limit_reached {
            summary.push_str(" · Result limit reached; narrow your search");
        }
        match self.skipped_entries {
            0 => {}
            1 => summary.push_str(" · 1 item could not be searched"),
            count => summary.push_str(&format!(" · {count} items could not be searched")),
        }
        summary
    }
}

pub fn recursive_search(
    vfs: &dyn Vfs,
    root: &VPath,
    options: &SearchOptions,
    cancel: &CancelToken,
) -> Result<SearchResults, String> {
    check_cancelled(cancel)?;
    if options.query.trim().is_empty() {
        return Err("Enter a name, path, or content query".to_owned());
    }
    if options
        .min_size
        .zip(options.max_size)
        .is_some_and(|(min, max)| min > max)
    {
        return Err("Minimum size must not exceed maximum size".to_owned());
    }
    if options.search_content && !(1..=MAX_CONTENT_BYTES).contains(&options.max_content_bytes) {
        return Err("Content scan limit must be between 1 byte and 64 MiB".to_owned());
    }
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
    let mut results = SearchResults::default();
    let mut pending = vec![(root.clone(), 0_usize)];
    'directories: while let Some((directory, depth)) = pending.pop() {
        check_cancelled(cancel)?;
        let entries = match vfs.read_dir(&directory, cancel) {
            Ok(entries) => entries,
            Err(error) => {
                check_cancelled(cancel)?;
                if directory == *root {
                    return Err(format!("Could not search {directory}: {error}"));
                }
                results.skip(error);
                continue;
            }
        };
        for entry in entries {
            check_cancelled(cancel)?;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    results.skip(error);
                    continue;
                }
            };
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
            // Traverse folders regardless of the result filters (e.g. *.rs).
            if entry.kind() == EntryKind::Directory
                && options.max_depth.is_none_or(|limit| depth < limit)
            {
                pending.push((path.clone(), depth + 1));
            }
            if options.kind.is_some_and(|kind| kind != entry.kind()) {
                continue;
            }
            if options.extension.as_ref().is_some_and(|expected| {
                !path
                    .as_path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case(expected.trim_start_matches('.'))
                    })
            }) {
                continue;
            }
            let metadata = match vfs.stat(&path, false) {
                Ok(metadata) => metadata,
                Err(error) => {
                    results.skip(error);
                    continue;
                }
            };
            if options.min_size.is_some_and(|min| metadata.size < min)
                || options.max_size.is_some_and(|max| metadata.size > max)
                || options.modified_after.is_some_and(|min| {
                    metadata
                        .modified
                        .is_none_or(|modified| modified.seconds < min)
                })
            {
                continue;
            }
            let name_matches = text_matches(
                &path.to_string(),
                &query,
                options.case_sensitive,
                regex.as_ref(),
            );
            // A name/path match needs no content I/O. Apply all facets before
            // opening files, and never read devices, sockets, or symbolic links.
            let content_match = if !name_matches
                && options.search_content
                && metadata.kind == EntryKind::File
                && entry.kind() == EntryKind::File
                && metadata.size <= options.max_content_bytes
            {
                match file_contains(
                    vfs,
                    &path,
                    &query,
                    options.case_sensitive,
                    regex.as_ref(),
                    options.max_content_bytes,
                    cancel,
                ) {
                    Ok(found) => found,
                    Err(error) => {
                        check_cancelled(cancel)?;
                        results.skip(format!("Could not read {path}: {error}"));
                        continue;
                    }
                }
            } else {
                false
            };
            if name_matches || content_match {
                results.hits.push(SearchHit {
                    path,
                    kind: entry.kind(),
                    size: metadata.size,
                    content_match,
                });
                if results.hits.len() >= MAX_SEARCH_RESULTS {
                    results.limit_reached = true;
                    break 'directories;
                }
            }
        }
    }
    check_cancelled(cancel)?;
    results
        .hits
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(results)
}

fn check_cancelled(cancel: &CancelToken) -> Result<(), String> {
    cancel.check().map_err(|_| "Search cancelled".to_owned())
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
    check_cancelled(cancel)?;
    let mut reader = vfs.open_read(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    let mut buffer = vec![0_u8; READ_BUFFER_SIZE];
    let mut remaining = max_bytes;
    while remaining > 0 {
        check_cancelled(cancel)?;
        let read_size = remaining.min(READ_BUFFER_SIZE as u64) as usize;
        let count = match reader.read(&mut buffer[..read_size]) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        };
        check_cancelled(cancel)?;
        if count == 0 {
            break;
        }
        let probe = count.min(8_192_usize.saturating_sub(bytes.len()));
        if buffer[..probe].contains(&0) {
            return Ok(false);
        }
        bytes.extend_from_slice(&buffer[..count]);
        remaining -= count as u64;
    }
    let text = String::from_utf8_lossy(&bytes);
    let found = text_matches(&text, query, case_sensitive, regex);
    check_cancelled(cancel)?;
    Ok(found)
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

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
