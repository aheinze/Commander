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
    marked: bool,
}

impl Selection {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            keys: BTreeSet::new(),
            anchor: None,
            marked: false,
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

    /// Marked files remain selected while the keyboard cursor moves.
    #[must_use]
    pub fn is_marked(&self) -> bool {
        self.marked && !self.keys.is_empty()
    }

    /// Record a batch-selection gesture, including one that marks only one file.
    pub fn mark(&mut self) {
        self.marked = !self.keys.is_empty();
    }

    /// Ordinary navigation replaces the selection; deliberate marks stay put.
    pub fn follow_cursor(&mut self, key: SelectionKey) {
        if !self.is_marked() {
            self.replace([key]);
        }
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.anchor = None;
        self.marked = false;
    }

    pub fn select(&mut self, key: SelectionKey) -> bool {
        self.anchor = Some(key.clone());
        let changed = self.keys.insert(key);
        self.marked |= self.keys.len() > 1;
        changed
    }

    pub fn select_preserving_anchor(&mut self, key: SelectionKey) -> bool {
        let changed = self.keys.insert(key);
        self.marked |= self.keys.len() > 1;
        changed
    }

    pub fn deselect(&mut self, key: &SelectionKey) -> bool {
        let removed = self.keys.remove(key);
        if self.anchor.as_ref() == Some(key) {
            self.anchor = None;
        }
        if self.keys.is_empty() {
            self.marked = false;
        }
        removed
    }

    pub fn toggle(&mut self, key: SelectionKey) -> bool {
        self.anchor = Some(key.clone());
        let selected = !self.keys.remove(&key);
        if selected {
            self.keys.insert(key);
        }
        self.mark();
        selected
    }

    pub fn set_anchor(&mut self, anchor: Option<SelectionKey>) {
        self.anchor = anchor;
    }

    pub fn retain_available(&mut self, available: &BTreeSet<SelectionKey>) {
        self.keys.retain(|key| available.contains(key));
        if self.keys.is_empty() {
            self.marked = false;
        }
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
        self.marked = self.keys.len() > 1;
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
        self.mark();
    }

    pub fn iter(&self) -> impl Iterator<Item = &SelectionKey> {
        self.keys.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
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

    #[test]
    fn cursor_selection_follows_focus_but_one_marked_file_stays_selected() {
        let one = SelectionKey::Path(VPath::from("/one"));
        let two = SelectionKey::Path(VPath::from("/two"));
        let mut selection = Selection::new();
        selection.select(one.clone());
        selection.follow_cursor(two.clone());
        assert_eq!(selection.iter().collect::<Vec<_>>(), vec![&two]);
        assert!(!selection.is_marked());
        selection.mark();
        selection.follow_cursor(one.clone());
        assert_eq!(selection.iter().collect::<Vec<_>>(), vec![&two]);
        selection.replace([one.clone()]);
        selection.follow_cursor(two.clone());
        assert_eq!(selection.iter().collect::<Vec<_>>(), vec![&two]);
        assert!(!selection.is_marked());
    }

    #[test]
    fn marks_survive_reduction_to_one_but_not_an_empty_selection() {
        let one = SelectionKey::Path(VPath::from("/one"));
        let two = SelectionKey::Path(VPath::from("/two"));
        let mut selection = Selection::new();
        selection.replace([one.clone(), two.clone()]);
        selection.retain_available(&BTreeSet::from([one.clone()]));
        selection.follow_cursor(two.clone());
        assert!(selection.is_marked() && selection.contains(&one));
        selection.deselect(&one);
        selection.follow_cursor(two.clone());
        assert!(!selection.is_marked() && selection.contains(&two));
        selection.clear();
        selection.toggle(one.clone());
        selection.follow_cursor(two);
        assert!(selection.is_marked() && selection.contains(&one));
    }
}
