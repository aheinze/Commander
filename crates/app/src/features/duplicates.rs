//! Size-first duplicate discovery. Symlinks and repeated hard links are excluded.
use dualpane_core::{CancelToken, EntryKind, Metadata, VPath};
use dualpane_vfs::Vfs;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug)]
pub struct DuplicateGroup {
    pub size: u64,
    pub paths: Vec<VPath>,
}
#[derive(Default, Debug)]
pub struct DuplicateResults {
    pub groups: Vec<DuplicateGroup>,
    pub scanned: usize,
    pub skipped: usize,
    pub warnings: Vec<String>,
}
impl DuplicateResults {
    pub fn reclaimable(&self) -> u64 {
        self.groups.iter().fold(0_u64, |sum, group| {
            sum.saturating_add(
                group
                    .size
                    .saturating_mul(group.paths.len().saturating_sub(1) as u64),
            )
        })
    }
    fn warning(&mut self, text: String) {
        self.skipped += 1;
        if self.warnings.len() < 20 {
            self.warnings.push(text);
        }
    }
}
pub fn find(
    vfs: &dyn Vfs,
    root: &VPath,
    hidden: bool,
    cancel: &CancelToken,
) -> Result<DuplicateResults, String> {
    let mut result = DuplicateResults::default();
    let mut sizes: BTreeMap<u64, Vec<(VPath, Metadata)>> = BTreeMap::new();
    let mut identities = HashSet::new();
    let mut directories = HashSet::new();
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        cancel
            .check()
            .map_err(|_| "Duplicate scan cancelled".to_owned())?;
        if !directories.insert(vfs.canonicalize(&directory).map_err(|e| e.to_string())?) {
            continue;
        }
        let entries = match vfs.read_dir(&directory, cancel) {
            Ok(entries) => entries,
            Err(error) => {
                result.warning(format!("{directory}: {error}"));
                continue;
            }
        };
        for entry in entries {
            cancel
                .check()
                .map_err(|_| "Duplicate scan cancelled".to_owned())?;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    result.warning(error.to_string());
                    continue;
                }
            };
            if !hidden && entry.name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            let path = directory.join_name(entry.name());
            let metadata = match vfs.stat(&path, false) {
                Ok(metadata) => metadata,
                Err(error) => {
                    result.warning(format!("{path}: {error}"));
                    continue;
                }
            };
            match metadata.kind {
                EntryKind::Directory => pending.push(path),
                EntryKind::File => {
                    if metadata.identity.is_some_and(|id| !identities.insert(id)) {
                        continue;
                    }
                    result.scanned += 1;
                    if result.scanned > 500_000 {
                        return Err("Scan exceeds 500,000 files. Choose a smaller folder.".into());
                    }
                    sizes
                        .entry(metadata.size)
                        .or_default()
                        .push((path, metadata));
                }
                _ => {}
            }
        }
    }
    for (size, candidates) in sizes.into_iter().filter(|(_, files)| files.len() > 1) {
        let mut hashes: BTreeMap<String, Vec<VPath>> = BTreeMap::new();
        for (path, before) in candidates {
            cancel
                .check()
                .map_err(|_| "Duplicate scan cancelled".to_owned())?;
            match super::sha256(vfs, &path, cancel).and_then(|hash| {
                let after = vfs.stat(&path, false).map_err(|e| e.to_string())?;
                if after.kind != before.kind
                    || after.identity != before.identity
                    || after.size != before.size
                    || after.modified != before.modified
                {
                    return Err("File changed during scan".into());
                }
                Ok(hash)
            }) {
                Ok(hash) => hashes.entry(hash).or_default().push(path),
                Err(error) => {
                    cancel
                        .check()
                        .map_err(|_| "Duplicate scan cancelled".to_owned())?;
                    result.warning(format!("{path}: {error}"));
                }
            }
        }
        for mut paths in hashes.into_values().filter(|paths| paths.len() > 1) {
            paths.sort();
            result.groups.push(DuplicateGroup { size, paths });
        }
    }
    result
        .groups
        .sort_by_key(|g| std::cmp::Reverse(g.size.saturating_mul(g.paths.len() as u64 - 1)));
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_scan_groups_content_not_names_and_excludes_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "same").unwrap();
        std::fs::write(dir.path().join("b"), "same").unwrap();
        std::fs::write(dir.path().join("c"), "else").unwrap();
        std::fs::write(dir.path().join(".hidden"), "same").unwrap();
        std::fs::hard_link(dir.path().join("a"), dir.path().join("hard")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
        let root = VPath::from(dir.path());
        let result = find(&dualpane_vfs::LocalFs, &root, false, &CancelToken::new()).unwrap();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].paths.len(), 2);
        assert_eq!(result.reclaimable(), 4);
        assert_eq!(
            find(&dualpane_vfs::LocalFs, &root, true, &CancelToken::new())
                .unwrap()
                .groups[0]
                .paths
                .len(),
            3
        );
        let cancelled = CancelToken::new();
        cancelled.cancel();
        assert!(find(&dualpane_vfs::LocalFs, &root, false, &cancelled).is_err());
    }
}
