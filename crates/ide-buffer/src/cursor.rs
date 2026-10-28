//! Caret and selection, counted in chars (not bytes, not grapheme clusters).

use std::ops::Range;

/// A selection: `anchor` is where it started, `head` is where the caret is.
///
/// An empty selection (`anchor == head`) is a caret.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub anchor: usize,
    pub head: usize,
}

impl Cursor {
    /// A caret at `char_idx` with nothing selected.
    pub fn caret(char_idx: usize) -> Self {
        Self {
            anchor: char_idx,
            head: char_idx,
        }
    }

    /// The selected range, always ordered from lower to higher index.
    pub fn selection(&self) -> Range<usize> {
        let (start, end) = if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        start..end
    }

    /// True when nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}
