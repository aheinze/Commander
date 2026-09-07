use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dualpane_core::{Selection, SelectionKey, VPath};
use dualpane_index::Listing;
use gio::prelude::*;
use gio::subclass::prelude::*;
use gtk::subclass::prelude::SelectionModelImpl;
use relm4::gtk;
use relm4::gtk::prelude::SelectionModelExt;
use relm4::gtk::{gio, glib};

type SelectionCallback = Box<dyn Fn(Selection, Option<u32>)>;

/// Lightweight object materialized only for rows requested by GTK's virtualized view.
#[derive(Clone, Debug)]
pub struct RowReference {
    pub listing: Arc<Listing>,
    pub row_index: u32,
    pub source_index: u32,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ListingListModel {
        pub listing: RefCell<Option<Arc<Listing>>>,
        pub published_items: Cell<u32>,
        pub visible_items: RefCell<HashMap<u32, glib::WeakRef<glib::BoxedAnyObject>>>,
        pub selection: RefCell<Selection>,
        pub selection_callback: RefCell<Option<SelectionCallback>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ListingListModel {
        const NAME: &'static str = "DualpaneListingListModel";
        type Type = super::ListingListModel;
        type Interfaces = (gio::ListModel, gtk::SelectionModel);
    }

    impl ObjectImpl for ListingListModel {}

    impl ListModelImpl for ListingListModel {
        fn item_type(&self) -> glib::Type {
            glib::BoxedAnyObject::static_type()
        }

        fn n_items(&self) -> u32 {
            self.published_items.get()
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            if let Some(item) = self
                .visible_items
                .borrow()
                .get(&position)
                .and_then(glib::WeakRef::upgrade)
            {
                return Some(item.upcast());
            }

            let listing = Arc::clone(self.listing.borrow().as_ref()?);
            let source_index = listing.source_index_at_row(position as usize)?;
            let item = glib::BoxedAnyObject::new(RowReference {
                listing,
                row_index: position,
                source_index,
            });
            let mut visible_items = self.visible_items.borrow_mut();
            if visible_items.len() > 4_096 {
                visible_items.retain(|_, weak| weak.upgrade().is_some());
            }
            visible_items.insert(position, item.downgrade());
            Some(item.upcast())
        }
    }

    impl SelectionModelImpl for ListingListModel {
        fn selection_in_range(&self, position: u32, n_items: u32) -> gtk::Bitset {
            let selected = gtk::Bitset::new_empty();
            if self.selection.borrow().is_empty() {
                return selected;
            }
            let end = position
                .saturating_add(n_items)
                .min(self.published_items.get());
            for row in position..end {
                if self.is_selected(row) {
                    selected.add(row);
                }
            }
            selected
        }

        fn is_selected(&self, position: u32) -> bool {
            self.selection_key(position)
                .is_some_and(|key| self.selection.borrow().contains(&key))
        }

        fn select_all(&self) -> bool {
            let keys =
                (0..self.published_items.get()).filter_map(|position| self.selection_key(position));
            self.selection.borrow_mut().replace(keys);
            self.publish_selection(0, self.published_items.get(), None);
            true
        }

        fn select_item(&self, position: u32, unselect_rest: bool) -> bool {
            let Some(key) = self.selection_key(position) else {
                return false;
            };
            let mut selection = self.selection.borrow_mut();
            let changed = if unselect_rest {
                let unchanged = selection.len() == 1 && selection.contains(&key);
                selection.clear();
                selection.select(key);
                !unchanged
            } else {
                selection.select(key)
            };
            drop(selection);
            if changed {
                self.publish_selection(0, self.published_items.get(), Some(position));
            } else {
                self.publish_selection(0, 0, Some(position));
            }
            changed
        }

        fn select_range(&self, position: u32, n_items: u32, unselect_rest: bool) -> bool {
            let end = position
                .saturating_add(n_items)
                .min(self.published_items.get());
            let keys: Vec<_> = (position..end)
                .filter_map(|row| self.selection_key(row))
                .collect();
            if keys.is_empty() {
                return false;
            }
            let mut selection = self.selection.borrow_mut();
            if unselect_rest {
                selection.clear();
            }
            let mut changed = false;
            for key in keys {
                changed |= selection.select_preserving_anchor(key);
            }
            drop(selection);
            if changed {
                self.publish_selection(position, end - position, end.checked_sub(1));
            } else {
                self.publish_selection(0, 0, end.checked_sub(1));
            }
            changed
        }

        fn set_selection(&self, selected: &gtk::Bitset, mask: &gtk::Bitset) -> bool {
            let item_count = self.published_items.get();
            let mut selection = self.selection.borrow_mut();
            let full_mask = item_count == 0
                || mask.size_in_range(0, item_count.saturating_sub(1)) == u64::from(item_count);
            if full_mask {
                let mut replacement = Selection::new();
                let mut selected_row = None;
                if let Some((iter, first)) = gtk::BitsetIter::init_first(selected) {
                    for position in std::iter::once(first).chain(iter) {
                        if position >= item_count {
                            break;
                        }
                        if let Some(key) = self.selection_key(position) {
                            replacement.select_preserving_anchor(key);
                            selected_row = Some(position);
                        }
                    }
                }
                let changed = *selection != replacement;
                *selection = replacement;
                drop(selection);
                if changed {
                    self.publish_selection(0, item_count, selected_row);
                } else if selected_row.is_some() {
                    self.publish_selection(0, 0, selected_row);
                }
                return changed;
            }

            let mut changed = false;
            let mut changed_row = None;
            let mut selected_row = None;
            let mut requested_selected_row = None;
            let Some((iter, first)) = gtk::BitsetIter::init_first(mask) else {
                return false;
            };
            for position in std::iter::once(first).chain(iter) {
                if position >= item_count {
                    break;
                }
                let Some(key) = self.selection_key(position) else {
                    continue;
                };
                let row_changed = if selected.contains(position) {
                    requested_selected_row = Some(position);
                    selection.select_preserving_anchor(key)
                } else {
                    selection.deselect(&key)
                };
                if row_changed {
                    changed = true;
                    changed_row = Some(position);
                    if selected.contains(position) {
                        selected_row = Some(position);
                    }
                }
            }
            drop(selection);
            let cursor_row = selected_row.or(changed_row).or(requested_selected_row);
            if changed {
                self.publish_selection(0, item_count, cursor_row);
            } else if cursor_row.is_some() {
                self.publish_selection(0, 0, cursor_row);
            }
            changed
        }

        fn unselect_all(&self) -> bool {
            if self.selection.borrow().is_empty() {
                return false;
            }
            self.selection.borrow_mut().clear();
            self.publish_selection(0, self.published_items.get(), None);
            true
        }

        fn unselect_item(&self, position: u32) -> bool {
            let Some(key) = self.selection_key(position) else {
                return false;
            };
            if !self.selection.borrow_mut().deselect(&key) {
                return false;
            }
            self.publish_selection(position, 1, Some(position));
            true
        }

        fn unselect_range(&self, position: u32, n_items: u32) -> bool {
            let end = position
                .saturating_add(n_items)
                .min(self.published_items.get());
            let keys: Vec<_> = (position..end)
                .filter_map(|row| self.selection_key(row))
                .collect();
            let mut selection = self.selection.borrow_mut();
            let mut changed = false;
            for key in keys {
                changed |= selection.deselect(&key);
            }
            drop(selection);
            if changed {
                self.publish_selection(position, end - position, Some(position));
            }
            changed
        }
    }

    impl ListingListModel {
        fn selection_key(&self, position: u32) -> Option<SelectionKey> {
            let listing = self.listing.borrow();
            let listing = listing.as_ref()?;
            let source_index = listing.source_index_at_row(position as usize)?;
            let entry = listing.entry(source_index)?;
            Some(SelectionKey::for_entry(listing.parent(), entry))
        }

        fn publish_selection(&self, position: u32, n_items: u32, cursor_row: Option<u32>) {
            if n_items > 0 {
                self.obj().selection_changed(position, n_items);
            }
            if let Some(callback) = self.selection_callback.borrow().as_ref() {
                callback(self.selection.borrow().clone(), cursor_row);
            }
        }
    }
}

glib::wrapper! {
    /// Custom `GListModel` backed by an immutable `Arc<Listing>` snapshot.
    pub struct ListingListModel(ObjectSubclass<imp::ListingListModel>)
        @implements gio::ListModel, gtk::SelectionModel;
}

impl ListingListModel {
    /// Creates an empty virtual model.
    #[must_use]
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Publishes a listing snapshot with the smallest valid GTK notification.
    pub fn set_listing(&self, listing: Option<Arc<Listing>>) {
        let old_items = self.n_items();
        let new_items = listing
            .as_ref()
            .map_or(0, |value| u32::try_from(value.len()).unwrap_or(u32::MAX));

        let previous = self.imp().listing.borrow().clone();
        if previous
            .as_ref()
            .zip(listing.as_ref())
            .is_some_and(|(previous, current)| Arc::ptr_eq(previous, current))
        {
            return;
        }
        if let Some(current) = listing.as_ref()
            && previous
                .as_ref()
                .is_some_and(|old| current.shares_rows_with(old))
        {
            self.update_metadata(Arc::clone(current));
            return;
        }
        let append_only = previous
            .as_ref()
            .zip(listing.as_ref())
            .is_some_and(|(previous, current)| current.is_append_only_successor_of(previous));

        *self.imp().listing.borrow_mut() = listing;
        self.imp().published_items.set(new_items);

        if append_only {
            let added = new_items.saturating_sub(old_items);
            if added > 0 {
                self.items_changed(old_items, 0, added);
            }
        } else {
            self.imp().visible_items.borrow_mut().clear();
            self.items_changed(0, old_items, new_items);
            if new_items > 0 && !self.imp().selection.borrow().is_empty() {
                self.selection_changed(0, new_items);
            }
        }
    }

    /// Rebinds only row objects that GTK currently keeps alive.
    pub fn refresh_visible(&self) {
        let mut positions: Vec<_> = self
            .imp()
            .visible_items
            .borrow()
            .iter()
            .filter_map(|(&position, weak)| weak.upgrade().map(|_| position))
            .collect();
        positions.sort_unstable();
        self.imp()
            .visible_items
            .borrow_mut()
            .retain(|position, weak| {
                weak.upgrade().is_some() && positions.binary_search(position).is_err()
            });
        self.notify_positions_changed(positions);
    }

    /// Rebinds visible files whose pending thumbnail work needs to be restarted.
    pub fn refresh_paths(&self, paths: &HashSet<VPath>) {
        let Some(listing) = self.imp().listing.borrow().clone() else {
            return;
        };
        let mut positions = self
            .imp()
            .visible_items
            .borrow()
            .iter()
            .filter_map(|(&position, weak)| {
                weak.upgrade()?;
                let entry = listing.row(position as usize)?;
                paths
                    .contains(&listing.parent().join_name(entry.name()))
                    .then_some(position)
            })
            .collect::<Vec<_>>();
        positions.sort_unstable();
        for position in &positions {
            self.imp().visible_items.borrow_mut().remove(position);
        }
        self.notify_positions_changed(positions);
    }

    /// Replaces metadata while notifying only row objects GTK currently keeps alive.
    pub fn update_metadata(&self, listing: Arc<Listing>) {
        let previous = self.imp().listing.borrow().clone();
        let mut positions: Vec<_> = self
            .imp()
            .visible_items
            .borrow()
            .iter()
            .filter_map(|(&position, weak)| {
                weak.upgrade()?;
                let source_index = listing.source_index_at_row(position as usize)?;
                let changed = previous.as_ref().is_none_or(|previous| {
                    previous.source_index_at_row(position as usize) != Some(source_index)
                        || previous.metadata(source_index) != listing.metadata(source_index)
                });
                changed.then_some(position)
            })
            .collect();
        positions.sort_unstable();
        *self.imp().listing.borrow_mut() = Some(listing);
        self.imp()
            .visible_items
            .borrow_mut()
            .retain(|position, weak| {
                weak.upgrade().is_some() && positions.binary_search(position).is_err()
            });

        self.notify_positions_changed(positions);
    }

    fn notify_positions_changed(&self, positions: Vec<u32>) {
        let mut ranges = Vec::new();
        for position in positions {
            match ranges.last_mut() {
                Some((_, end)) if *end == position => *end += 1,
                _ => ranges.push((position, position + 1)),
            }
        }
        for (start, end) in ranges {
            let count = end - start;
            self.items_changed(start, count, count);
        }
    }

    /// Returns the current immutable snapshot.
    #[must_use]
    pub fn listing(&self) -> Option<Arc<Listing>> {
        self.imp().listing.borrow().clone()
    }

    /// Installs the pane callback invoked after pointer or GTK selection changes.
    pub fn set_selection_callback(&self, callback: impl Fn(Selection, Option<u32>) + 'static) {
        *self.imp().selection_callback.borrow_mut() = Some(Box::new(callback));
    }

    /// Replaces stable selection without changing the current listing.
    pub fn set_stable_selection(&self, selection: &Selection) {
        if self.imp().selection.borrow().eq(selection) {
            return;
        }
        *self.imp().selection.borrow_mut() = selection.clone();
        let items = self.n_items();
        if items > 0 {
            self.selection_changed(0, items);
        }
    }

    /// Returns a snapshot of the stable selection.
    #[must_use]
    pub fn stable_selection(&self) -> Selection {
        self.imp().selection.borrow().clone()
    }
}

impl Default for ListingListModel {
    fn default() -> Self {
        Self::new()
    }
}
