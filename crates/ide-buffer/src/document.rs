//! The editable text of one file.
//!
//! Positions are char indices, which is what the editor widget and the language
//! servers can both agree on: LSP positions are UTF-16 units, but converting at
//! the boundary is cheaper than keeping two coordinate systems inside the buffer.

use std::ops::Range;

use ropey::Rope;
use ropey::RopeSlice;

use crate::history::{Edit, EditKind, HISTORY_LIMIT, History};

/// One open file's text plus its undo history.
#[derive(Debug)]
pub struct Document {
    rope: Rope,
    history: History,
    dirty: bool,
}

impl Document {
    /// A document over `text`. Nothing counts as a change yet.
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            history: History::new(HISTORY_LIMIT),
            dirty: false,
        }
    }

    /// The whole text as one string. Allocates; use for saving and tests.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Length in chars.
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Inserts `text` at `char_idx`. A position past the end appends.
    pub fn insert(&mut self, char_idx: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let at = char_idx.min(self.rope.len_chars());
        self.rope.insert(at, text);
        self.history.record(Edit {
            range: at..at,
            removed: String::new(),
            inserted: text.to_string(),
            kind: EditKind::Insert,
        });
        self.dirty = true;
    }

    /// Removes a range of chars. The range is clamped to the document.
    pub fn remove(&mut self, range: Range<usize>) {
        let len = self.rope.len_chars();
        let start = range.start.min(len);
        let end = range.end.min(len).max(start);
        if start == end {
            return;
        }
        let removed = self.rope.slice(start..end).to_string();
        self.rope.remove(start..end);
        self.history.record(Edit {
            range: start..end,
            removed,
            inserted: String::new(),
            kind: EditKind::Remove,
        });
        self.dirty = true;
    }

    /// Reverts the last step. Returns false when there is nothing to revert.
    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.history.pop_undo() else {
            return false;
        };
        match edit.kind {
            EditKind::Insert => {
                let end = edit.range.start + edit.inserted.chars().count();
                self.rope.remove(edit.range.start..end);
            }
            EditKind::Remove => {
                self.rope.insert(edit.range.start, &edit.removed);
            }
        }
        self.history.push_redo(edit);
        self.dirty = true;
        true
    }

    /// Replays the last reverted step.
    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.history.pop_redo() else {
            return false;
        };
        match edit.kind {
            EditKind::Insert => {
                self.rope.insert(edit.range.start, &edit.inserted);
            }
            EditKind::Remove => {
                let end = edit.range.start + edit.removed.chars().count();
                self.rope.remove(edit.range.start..end);
            }
        }
        self.history.push_undo(edit);
        self.dirty = true;
        true
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// True when the text differs from the last saved state.
    ///
    /// Undoing back to the saved content still reports a change: the flag is
    /// cleared only by [`Document::mark_saved`]. That errs towards warning the
    /// user about a close, never towards losing text.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Called after the text has been written to disk.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Number of lines; an empty document has one empty line.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// One line, without the trailing newline. `None` when out of range.
    pub fn line(&self, idx: usize) -> Option<RopeSlice<'_>> {
        if idx >= self.rope.len_lines() {
            return None;
        }
        let slice = self.rope.line(idx);
        let mut end = slice.len_chars();
        // Ropey ends a line with `\n`, `\r\n` or a lone `\r`; only the last two
        // characters can be a terminator, so at most two are dropped.
        for _ in 0..2 {
            if end > 0 && matches!(slice.char(end - 1), '\n' | '\r') {
                end -= 1;
            }
        }
        Some(slice.slice(..end))
    }

    /// Byte offset of the first character of a line.
    ///
    /// Ropey breaks lines on `\n`, `\r\n` and a lone `\r`, so callers must ask
    /// the document rather than scan for `\n` themselves: a stray `\r` in the
    /// text would make the two views of the line disagree.
    pub fn line_start_byte(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return self.rope.len_bytes();
        }
        self.rope.line_to_byte(line)
    }

    /// Line and column (both zero based, in chars) of a char index.
    pub fn char_to_line_col(&self, char_idx: usize) -> (usize, usize) {
        let idx = char_idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        let line_start = self.rope.line_to_char(line);
        (line, idx - line_start)
    }

    /// Char index of a line and column, clamped to the end of the text.
    pub fn line_col_to_char(&self, line: usize, col: usize) -> usize {
        if line >= self.rope.len_lines() {
            return self.rope.len_chars();
        }
        let line_start = self.rope.line_to_char(line);
        (line_start + col).min(self.rope.len_chars())
    }
}
