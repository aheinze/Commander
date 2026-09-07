use std::collections::BTreeSet;

use crate::{Entry, FileIdentity, VPath};

/// Stable key used to preserve selection across row-order and view changes.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SelectionKey {
    Identity(FileIdentity),
    Path(VPath),
}

impl SelectionKey {
    /// Builds the strongest stable key available for an entry.
    #[must_use]
    pub fn for_entry(parent: &VPath, entry: &Entry) -> Self {
        entry.identity().map_or_else(
            || Self::Path(parent.join_name(entry.name())),
            Self::Identity,
        )
    }
}

/// Pane-local selection independent of row position.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Selection {
    keys: BTreeSet<SelectionKey>,
    anchor: Option<SelectionKey>,
}

impl Selection {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            keys: BTreeSet::new(),
            anchor: None,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    #[must_use]
    pub fn contains(&self, key: &SelectionKey) -> bool {
        self.keys.contains(key)
    }

    #[must_use]
    pub fn anchor(&self) -> Option<&SelectionKey> {
        self.anchor.as_ref()
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.anchor = None;
    }

    pub fn select(&mut self, key: SelectionKey) -> bool {
        self.anchor = Some(key.clone());
        self.keys.insert(key)
    }

    pub fn select_preserving_anchor(&mut self, key: SelectionKey) -> bool {
        self.keys.insert(key)
    }

    pub fn deselect(&mut self, key: &SelectionKey) -> bool {
        let removed = self.keys.remove(key);
        if self.anchor.as_ref() == Some(key) {
            self.anchor = None;
        }
        removed
    }

    pub fn toggle(&mut self, key: SelectionKey) -> bool {
        self.anchor = Some(key.clone());
        if self.keys.remove(&key) {
            false
        } else {
            self.keys.insert(key);
            true
        }
    }

    pub fn set_anchor(&mut self, anchor: Option<SelectionKey>) {
        self.anchor = anchor;
    }

    pub fn retain_available(&mut self, available: &BTreeSet<SelectionKey>) {
        self.keys.retain(|key| available.contains(key));
        if self
            .anchor
            .as_ref()
            .is_some_and(|anchor| !available.contains(anchor))
        {
            self.anchor = None;
        }
    }

    pub fn replace<I>(&mut self, keys: I)
    where
        I: IntoIterator<Item = SelectionKey>,
    {
        self.keys = keys.into_iter().collect();
        self.anchor = self.keys.last().cloned();
    }

    pub fn invert<I>(&mut self, available: I)
    where
        I: IntoIterator<Item = SelectionKey>,
    {
        let available: BTreeSet<_> = available.into_iter().collect();
        self.keys = available
            .into_iter()
            .filter(|key| !self.keys.contains(key))
            .collect();
        self.anchor = self.keys.last().cloned();
    }

    pub fn iter(&self) -> impl Iterator<Item = &SelectionKey> {
        self.keys.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use crate::{Entry, EntryKind, FileIdentity, VPath};

    use super::{Selection, SelectionKey};

    #[test]
    fn identity_selection_survives_a_path_change() {
        let identity = FileIdentity {
            device: 7,
            inode: 42,
        };
        let before = Entry::new(OsString::from("before"), EntryKind::File, Some(identity));
        let after = Entry::new(OsString::from("after"), EntryKind::File, Some(identity));
        let mut selection = Selection::new();

        selection.select(SelectionKey::for_entry(&VPath::from("/left"), &before));

        assert!(selection.contains(&SelectionKey::for_entry(&VPath::from("/right"), &after)));
    }

    #[test]
    fn path_fallback_and_invert_are_stable() {
        let parent = VPath::from("/tmp");
        let one = Entry::new(OsString::from("one"), EntryKind::File, None);
        let two = Entry::new(OsString::from("two"), EntryKind::File, None);
        let one = SelectionKey::for_entry(&parent, &one);
        let two = SelectionKey::for_entry(&parent, &two);
        let mut selection = Selection::new();
        selection.select(one.clone());

        selection.invert([one.clone(), two.clone()]);

        assert!(!selection.contains(&one));
        assert!(selection.contains(&two));
    }
}
