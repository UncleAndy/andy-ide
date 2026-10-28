//! Undo and redo stacks.
//!
//! Consecutive typing is folded into one step, so `Ctrl+Z` after typing a word
//! removes the word rather than one character.

use std::collections::VecDeque;
use std::ops::Range;

/// How many undo steps a document keeps.
pub const HISTORY_LIMIT: usize = 1000;

/// What an edit did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Insert,
    Remove,
}

/// One reversible change, in char indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Range in the document as it was before the edit.
    pub range: Range<usize>,
    /// Text the edit took out.
    pub removed: String,
    /// Text the edit put in.
    pub inserted: String,
    pub kind: EditKind,
}

/// Two stacks with a bounded depth.
#[derive(Debug)]
pub struct History {
    undo: VecDeque<Edit>,
    redo: Vec<Edit>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(HISTORY_LIMIT)
    }
}

impl History {
    /// A history that keeps at most `limit` steps.
    pub fn new(limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Records a fresh edit. Editing after an undo drops the redo stack, as in
    /// every other editor.
    pub fn record(&mut self, edit: Edit) {
        self.redo.clear();
        if let Some(last) = self.undo.back_mut()
            && can_coalesce(last, &edit)
        {
            last.inserted.push_str(&edit.inserted);
            return;
        }
        self.push_undo(edit);
    }

    /// Records an edit that must stay its own step (replayed redo).
    pub fn push_undo(&mut self, edit: Edit) {
        self.undo.push_back(edit);
        while self.undo.len() > self.limit {
            self.undo.pop_front();
        }
    }

    pub fn pop_undo(&mut self) -> Option<Edit> {
        self.undo.pop_back()
    }

    pub fn push_redo(&mut self, edit: Edit) {
        self.redo.push(edit);
    }

    pub fn pop_redo(&mut self) -> Option<Edit> {
        self.redo.pop()
    }
}

/// A pure insert directly at the end of the previous pure insert extends it.
fn can_coalesce(last: &Edit, next: &Edit) -> bool {
    last.kind == EditKind::Insert
        && next.kind == EditKind::Insert
        && last.removed.is_empty()
        && next.removed.is_empty()
        && last.range.start + last.inserted.chars().count() == next.range.start
}
