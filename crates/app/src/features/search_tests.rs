use super::*;
use std::io::{Cursor, Seek, SeekFrom};
use std::sync::atomic::{AtomicUsize, Ordering};

use dualpane_core::{Capabilities, Entry, Metadata, SizeHint};
use dualpane_vfs::{LocalFs, ReadSeek, Result as VfsResult, VfsError, WriteSeek};

#[derive(Default)]
struct FaultFs {
    denied_dir: Option<VPath>,
    denied_read: Option<VPath>,
    denied_stat: Option<VPath>,
    entry_error: bool,
    cancel_on_read: Option<CancelToken>,
    synthetic_entries: Option<usize>,
    opens: AtomicUsize,
}

fn denied(path: &VPath) -> VfsError {
    VfsError::Io {
        operation: "test access",
        path: path.clone(),
        source: std::io::ErrorKind::PermissionDenied.into(),
    }
}

impl Vfs for FaultFs {
    fn read_dir(
        &self,
        path: &VPath,
        cancel: &CancelToken,
    ) -> VfsResult<Box<dyn Iterator<Item = VfsResult<Entry>> + Send>> {
        if self.denied_dir.as_ref() == Some(path) {
            return Err(denied(path));
        }
        if let Some(count) = self.synthetic_entries {
            return Ok(Box::new((0..count).rev().map(|index| {
                Ok(Entry::new(
                    format!("hit-{index:05}").into(),
                    EntryKind::File,
                    None,
                ))
            })));
        }
        let entries = LocalFs.read_dir(path, cancel)?;
        if self.entry_error {
            return Ok(Box::new(std::iter::once(Err(denied(path))).chain(entries)));
        }
        Ok(entries)
    }
    fn stat(&self, path: &VPath, follow: bool) -> VfsResult<Metadata> {
        if self.denied_stat.as_ref() == Some(path) {
            return Err(denied(path));
        }
        if self.synthetic_entries.is_some() {
            let mut metadata = LocalFs.stat(&path.parent().unwrap(), false)?;
            metadata.kind = EntryKind::File;
            return Ok(metadata);
        }
        LocalFs.stat(path, follow)
    }
    fn open_read(&self, path: &VPath) -> VfsResult<Box<dyn ReadSeek>> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        if self.denied_read.as_ref() == Some(path) {
            return Err(denied(path));
        }
        if let Some(cancel) = &self.cancel_on_read {
            return Ok(Box::new(CancellingReader {
                data: Cursor::new(vec![b'x'; READ_BUFFER_SIZE * 3]),
                cancel: cancel.clone(),
                reads: 0,
            }));
        }
        LocalFs.open_read(path)
    }
    fn create_write(&self, _: &VPath, _: SizeHint) -> VfsResult<Box<dyn WriteSeek>> {
        unreachable!("search is read only")
    }
    fn rename(&self, _: &VPath, _: &VPath) -> VfsResult<()> {
        unreachable!("search is read only")
    }
    fn remove(&self, _: &VPath, _: EntryKind) -> VfsResult<()> {
        unreachable!("search is read only")
    }
    fn capabilities(&self) -> Capabilities {
        LocalFs.capabilities()
    }
}

struct CancellingReader {
    data: Cursor<Vec<u8>>,
    cancel: CancelToken,
    reads: usize,
}
impl Read for CancellingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.reads += 1;
        assert_eq!(
            self.reads, 1,
            "a cancelled search must not request the next chunk"
        );
        self.cancel.cancel();
        self.data.read(buffer)
    }
}
impl Seek for CancellingReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.data.seek(position)
    }
}

fn options() -> SearchOptions {
    SearchOptions {
        query: "needle".into(),
        search_content: true,
        ..SearchOptions::default()
    }
}

#[test]
fn unreadable_files_folders_and_entries_retain_accessible_results() {
    let fixture = tempfile::tempdir().unwrap();
    let root = VPath::from(fixture.path());
    std::fs::create_dir(fixture.path().join("private")).unwrap();
    std::fs::write(fixture.path().join("private/secret.txt"), "needle").unwrap();
    std::fs::write(fixture.path().join("blocked.txt"), "needle").unwrap();
    std::fs::write(fixture.path().join("gone.txt"), "needle").unwrap();
    std::fs::write(fixture.path().join("readable.txt"), "a needle in text").unwrap();
    let vfs = FaultFs {
        denied_dir: Some(root.join_name("private".as_ref())),
        denied_read: Some(root.join_name("blocked.txt".as_ref())),
        denied_stat: Some(root.join_name("gone.txt".as_ref())),
        entry_error: true,
        ..FaultFs::default()
    };
    let result = recursive_search(&vfs, &root, &options(), &CancelToken::new()).unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].path.file_name().unwrap(), "readable.txt");
    assert!(result.hits[0].content_match);
    assert_eq!(result.skipped_entries, 4);
    assert!(result.first_error.unwrap().contains("test access"));
    assert!(!result.limit_reached);
}

#[test]
fn inaccessible_root_and_invalid_queries_are_errors() {
    let fixture = tempfile::tempdir().unwrap();
    let root = VPath::from(fixture.path());
    let vfs = FaultFs {
        denied_dir: Some(root.clone()),
        ..FaultFs::default()
    };
    assert!(
        recursive_search(&vfs, &root, &options(), &CancelToken::new())
            .unwrap_err()
            .contains("Could not search")
    );
    for invalid in [
        SearchOptions {
            query: "  ".into(),
            ..options()
        },
        SearchOptions {
            query: "[".into(),
            regex: true,
            ..options()
        },
        SearchOptions {
            min_size: Some(10),
            max_size: Some(2),
            ..options()
        },
        SearchOptions {
            max_content_bytes: MAX_CONTENT_BYTES + 1,
            ..options()
        },
    ] {
        assert!(recursive_search(&LocalFs, &root, &invalid, &CancelToken::new()).is_err());
    }
}

#[test]
fn filters_run_before_content_reads_and_do_not_prune_matching_descendants() {
    let fixture = tempfile::tempdir().unwrap();
    let root = VPath::from(fixture.path());
    std::fs::create_dir(fixture.path().join("nested")).unwrap();
    std::fs::write(fixture.path().join("nested/allowed.rs"), "needle").unwrap();
    std::fs::write(fixture.path().join("blocked.txt"), "needle").unwrap();
    std::fs::write(fixture.path().join("small.rs"), "x").unwrap();
    let vfs = FaultFs {
        denied_read: Some(root.join_name("blocked.txt".as_ref())),
        ..FaultFs::default()
    };
    let result = recursive_search(
        &vfs,
        &root,
        &SearchOptions {
            extension: Some(".RS".into()),
            min_size: Some(2),
            kind: Some(EntryKind::File),
            ..options()
        },
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].path.file_name().unwrap(), "allowed.rs");
    assert_eq!(result.skipped_entries, 0);
    assert_eq!(vfs.opens.load(Ordering::Relaxed), 1);
}

#[test]
fn content_search_checks_cancellation_between_reads() {
    let fixture = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("content.txt"), "text").unwrap();
    let cancel = CancelToken::new();
    let vfs = FaultFs {
        cancel_on_read: Some(cancel.clone()),
        ..FaultFs::default()
    };
    assert_eq!(
        recursive_search(&vfs, &VPath::from(fixture.path()), &options(), &cancel).unwrap_err(),
        "Search cancelled"
    );
}

#[test]
fn results_at_the_limit_are_sorted_and_explicitly_incomplete() {
    let fixture = tempfile::tempdir().unwrap();
    let vfs = FaultFs {
        synthetic_entries: Some(MAX_SEARCH_RESULTS + 5),
        ..FaultFs::default()
    };
    let result = recursive_search(
        &vfs,
        &VPath::from(fixture.path()),
        &SearchOptions {
            query: "hit-".into(),
            ..SearchOptions::default()
        },
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(result.hits.len(), MAX_SEARCH_RESULTS);
    assert!(result.limit_reached);
    assert!(result.summary().contains("narrow your search"));
    assert!(
        result
            .hits
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
    );
}

#[test]
fn unicode_and_regex_matches_span_read_boundaries_and_binary_files_are_skipped() {
    let fixture = tempfile::tempdir().unwrap();
    let root = VPath::from(fixture.path());
    let mut content = "x".repeat(READ_BUFFER_SIZE - 1);
    content.push_str("Éclair\nneedle");
    std::fs::write(fixture.path().join("content.txt"), content).unwrap();
    std::fs::write(fixture.path().join("binary.bin"), b"\0\0needle").unwrap();
    for query in ["éclair", "Éclair\\nneedle"] {
        let result = recursive_search(
            &LocalFs,
            &root,
            &SearchOptions {
                query: query.into(),
                regex: true,
                ..options()
            },
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].path.file_name().unwrap(), "content.txt");
    }
    let result = recursive_search(&LocalFs, &root, &options(), &CancelToken::new()).unwrap();
    assert_eq!(result.hits.len(), 1);
}

#[cfg(unix)]
#[test]
fn depth_hidden_ignored_and_symlink_filters_respect_the_search_scope() {
    let fixture = tempfile::tempdir().unwrap();
    for dir in ["nested", ".hidden", "node_modules"] {
        std::fs::create_dir(fixture.path().join(dir)).unwrap();
        std::fs::write(fixture.path().join(dir).join("needle.txt"), "needle").unwrap();
    }
    std::os::unix::fs::symlink(fixture.path(), fixture.path().join("needle-loop")).unwrap();
    let root = VPath::from(fixture.path());
    let run = |options| recursive_search(&LocalFs, &root, &options, &CancelToken::new()).unwrap();
    assert_eq!(run(options()).hits.len(), 2);
    assert_eq!(
        run(SearchOptions {
            include_symlinks: false,
            ..options()
        })
        .hits
        .len(),
        1
    );
    assert_eq!(
        run(SearchOptions {
            max_depth: Some(0),
            ..options()
        })
        .hits
        .len(),
        1
    );
    assert_eq!(
        run(SearchOptions {
            include_hidden: true,
            include_ignored: true,
            ..options()
        })
        .hits
        .len(),
        4
    );
}
