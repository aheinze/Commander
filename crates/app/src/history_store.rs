//! Undo state uses the session worker's ordered I/O queue and durable atomic snapshots.
use crate::app::{HistoryDirection, HistoryEntry};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct HistoryState {
    pub undo: Vec<HistoryEntry>,
    pub redo: Vec<HistoryEntry>,
    pub pending: Option<(HistoryEntry, HistoryDirection)>,
}

pub(crate) fn path() -> Option<PathBuf> {
    crate::session::state_directory().map(|directory| directory.join("history.json"))
}

pub(crate) fn save(path: &Path, state: &HistoryState) -> Result<(), String> {
    write_atomic(
        path,
        &serde_json::to_vec(state).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn load(path: &Path) -> (HistoryState, Option<String>) {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (HistoryState::default(), None);
        }
        Err(error) => return (HistoryState::default(), Some(error.to_string())),
    };
    match serde_json::from_slice::<HistoryState>(&bytes) {
        Ok(state) if state.pending.is_none() => (state, None),
        result => {
            let archive = path.with_file_name(format!(
                "interrupted-history-{}.json",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            let reason = result.err().map_or_else(
                || "An undo or redo was interrupted".to_owned(),
                |error| format!("Undo history could not be read: {error}"),
            );
            let preserved = write_atomic(&archive, &bytes).map_or_else(
                |error| {
                    format!(
                        "Could not archive it: {error}; original is at {}",
                        path.display()
                    )
                },
                |()| format!("Its recovery record is at {}", archive.display()),
            );
            (
                HistoryState::default(),
                Some(format!(
                    "{reason}. {preserved}. Check the affected files before making further changes."
                )),
            )
        }
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing state directory"))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".commander-state-")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualpane_core::VPath;
    #[test]
    fn pending_history_is_preserved_for_review_instead_of_replayed() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("history.json");
        let entry = HistoryEntry::Rename {
            from: VPath::from("/before"),
            to: VPath::from("/after"),
        };
        let history = HistoryState {
            undo: Vec::new(),
            redo: Vec::new(),
            pending: Some((entry, HistoryDirection::Undo)),
        };
        save(&file, &history).unwrap();
        let bytes = fs::read(&file).unwrap();
        let (loaded, warning) = load(&file);
        assert!(loaded.undo.is_empty() && loaded.redo.is_empty() && loaded.pending.is_none());
        assert!(warning.unwrap().contains("interrupted"));
        let archived = fs::read_dir(fixture.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("interrupted-history-")
            })
            .unwrap();
        assert_eq!(fs::read(archived.path()).unwrap(), bytes);
    }
}
