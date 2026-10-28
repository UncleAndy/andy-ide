//! Open tabs.
//!
//! This is the model only: it knows which files are open, in which order, which
//! one is active, and which of them have unsaved changes. It deliberately does
//! not depend on `ide-buffer`: the interface layer owns buffers and reports their
//! dirtiness here by buffer id, so this crate stays free of the text stack.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One open tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabItem {
    pub path: PathBuf,
    /// Identifier the interface layer gave to the buffer behind this tab.
    pub buffer_id: usize,
}

/// The set of open tabs and which one is active.
#[derive(Debug, Clone, Default)]
pub struct Tabs {
    items: Vec<TabItem>,
    active: Option<usize>,
    dirty: HashMap<usize, bool>,
}

impl Tabs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens `path`, or activates it when it is already open.
    ///
    /// Returns the index of the tab. A new tab is appended and becomes active.
    pub fn open(&mut self, path: PathBuf, buffer_id: usize) -> usize {
        if let Some(index) = self.items.iter().position(|item| item.path == path) {
            self.active = Some(index);
            return index;
        }
        self.items.push(TabItem { path, buffer_id });
        let index = self.items.len() - 1;
        self.active = Some(index);
        index
    }

    /// Closes a tab unless it has unsaved changes.
    ///
    /// Returns false when the caller must ask the user first; then
    /// [`Tabs::close_forced`] closes it.
    pub fn close(&mut self, index: usize) -> bool {
        if self.dirty(index) {
            return false;
        }
        self.close_forced(index);
        true
    }

    /// Closes a tab regardless of unsaved changes.
    pub fn close_forced(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        let removed = self.items.remove(index);
        self.dirty.remove(&removed.buffer_id);

        // After removing, the tab that was to the right now sits at `index`;
        // when the last tab was closed, the one to the left is the neighbour.
        let len = self.items.len();
        self.active = match self.active {
            _ if len == 0 => None,
            Some(active) if active == index => Some(index.min(len - 1)),
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
    }

    /// The active tab, if any.
    pub fn active(&self) -> Option<usize> {
        self.active.filter(|index| *index < self.items.len())
    }

    /// Activates a tab; an out-of-range index is ignored.
    pub fn set_active(&mut self, index: usize) {
        if index < self.items.len() {
            self.active = Some(index);
        }
    }

    /// All tabs, in display order.
    pub fn items(&self) -> &[TabItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Records whether the buffer behind a tab has unsaved changes.
    pub fn dirty_flag(&mut self, buffer_id: usize, dirty: bool) {
        if dirty {
            self.dirty.insert(buffer_id, true);
        } else {
            self.dirty.remove(&buffer_id);
        }
    }

    /// True when the tab at `index` has unsaved changes.
    pub fn dirty(&self, index: usize) -> bool {
        self.items
            .get(index)
            .and_then(|item| self.dirty.get(&item.buffer_id))
            .copied()
            .unwrap_or(false)
    }

    /// Index of the tab showing `path`.
    pub fn find(&self, path: &Path) -> Option<usize> {
        self.items.iter().position(|item| item.path == path)
    }
}
