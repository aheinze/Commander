//! Preflight feedback; the rename engine revalidates immediately before writing.
use dualpane_core::VPath;
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
};

pub fn validate(items: &[(VPath, String)]) -> Vec<Option<String>> {
    let sources: HashSet<_> = items.iter().map(|(source, _)| source.clone()).collect();
    let targets: Vec<_> = items
        .iter()
        .map(|(source, name)| source.parent().map(|p| p.join_name(OsStr::new(name))))
        .collect();
    let mut counts = HashMap::new();
    for target in targets.iter().flatten() {
        *counts.entry(target).or_insert(0_usize) += 1;
    }
    items
        .iter()
        .zip(&targets)
        .map(|((source, name), target)| {
            if source.file_name().and_then(OsStr::to_str).is_none() {
                return Some("Filename is not valid UTF-8".into());
            }
            if name.is_empty() || matches!(name.as_str(), "." | "..") || name.contains(['/', '\0'])
            {
                return Some("Invalid filename".into());
            }
            let Some(target) = target else {
                return Some("Cannot rename a filesystem root".into());
            };
            if counts.get(target).copied().unwrap_or(0) > 1 {
                return Some("Duplicate destination name".into());
            }
            match std::fs::symlink_metadata(target.as_path()) {
                Ok(_) if !sources.contains(target) => Some("Destination already exists".into()),
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Some(e.to_string()),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rename_preview_detects_conflicts_but_allows_swaps() {
        let temp = tempfile::tempdir().unwrap();
        let a = VPath::from(temp.path().join("a"));
        let b = VPath::from(temp.path().join("b"));
        std::fs::write(a.as_path(), "a").unwrap();
        std::fs::write(b.as_path(), "b").unwrap();
        assert!(
            validate(&[(a.clone(), "b".into()), (b.clone(), "a".into())])
                .iter()
                .all(Option::is_none)
        );
        assert!(validate(&[(a.clone(), "b".into())])[0].is_some());
        assert!(
            validate(&[(a.clone(), "x".into()), (b, "x".into())])
                .iter()
                .all(Option::is_some)
        );
        assert!(validate(&[(a, "../escape".into())])[0].is_some());
    }
}
