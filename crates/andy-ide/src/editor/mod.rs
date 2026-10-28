//! The text editor: one buffer per open tab.

pub mod edit;
pub mod view;

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ide_buffer::{Cursor, Document};
use ide_syntax::{Highlighter, Language};

use self::edit::Motion;

/// An open document: the rope, the caret, and the highlighter.
///
/// The document is the source of truth; `text` is a flat mirror of it, rebuilt
/// after every change, because both the highlighter and the renderer work on
/// byte offsets into the whole file.
pub struct EditorBuffer {
    pub path: PathBuf,
    pub doc: Document,
    pub cursor: Cursor,
    pub highlighter: Option<Highlighter>,
    /// Column remembered across vertical moves.
    goal_col: Option<usize>,
    text: String,
    line_starts: Vec<usize>,
}

impl EditorBuffer {
    /// Reads a file relative to the project root.
    pub fn load(root: &Path, rel: &Path) -> Result<Self> {
        let absolute = root.join(rel);
        let text = std::fs::read_to_string(&absolute)
            .with_context(|| format!("cannot read {}", absolute.display()))?;
        Ok(Self::from_text(rel.to_path_buf(), text))
    }

    /// Builds a buffer from text; the language comes from the path and shebang.
    pub fn from_text(path: PathBuf, text: String) -> Self {
        let first_line = text.lines().next();
        let language = Language::detect(&path, first_line);
        let highlighter = language.map(|language| {
            let mut highlighter = Highlighter::new(language);
            highlighter.set_text(&text);
            highlighter
        });

        let mut buffer = Self {
            path,
            doc: Document::new(&text),
            cursor: Cursor::caret(0),
            highlighter,
            goal_col: None,
            text: String::new(),
            line_starts: Vec::new(),
        };
        buffer.refresh();
        buffer
    }

    /// Writes the buffer back to disk and marks it clean.
    pub fn save(&mut self, root: &Path) -> Result<()> {
        let absolute = root.join(&self.path);
        ide_core::fs::write_atomic(&absolute, self.text.as_bytes())
            .with_context(|| format!("cannot write {}", absolute.display()))?;
        self.doc.mark_saved();
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.doc.is_dirty()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte offset of the first character of each line.
    pub fn line_starts(&self) -> &[usize] {
        &self.line_starts
    }

    /// One line without its terminator; a trailing carriage return is dropped.
    pub fn line_text(&self, line: usize) -> &str {
        let Some(&start) = self.line_starts.get(line) else {
            return "";
        };
        let end = self
            .line_starts
            .get(line + 1)
            .map(|next| next - 1)
            .unwrap_or(self.text.len())
            .min(self.text.len());
        let slice = &self.text[start.min(end)..end];
        slice.strip_suffix('\r').unwrap_or(slice)
    }

    /// Width of the longest line in characters, for the horizontal extent.
    pub fn max_line_chars(&self) -> usize {
        (0..self.line_count())
            .map(|line| self.line_text(line).chars().count())
            .max()
            .unwrap_or(0)
    }

    pub fn line_col(&self) -> (usize, usize) {
        self.doc.char_to_line_col(self.caret())
    }

    pub fn caret(&self) -> usize {
        self.cursor.head
    }

    /// Character index for a line and column, clamped to the line.
    pub fn char_index(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.line_count().saturating_sub(1));
        let col = col.min(self.line_text(line).chars().count());
        self.doc.line_col_to_char(line, col)
    }

    pub fn set_caret(&mut self, caret: usize) {
        self.cursor = Cursor::caret(caret.min(self.doc.len_chars()));
        self.goal_col = None;
    }

    pub fn set_drag_caret(&mut self, caret: usize) {
        self.cursor.head = caret.min(self.doc.len_chars());
    }

    pub fn selection(&self) -> Range<usize> {
        self.cursor.selection()
    }

    pub fn has_selection(&self) -> bool {
        !self.cursor.is_empty()
    }

    /// Moves the caret; `extend` keeps the anchor, so the move selects.
    pub fn move_caret(&mut self, motion: Motion, extend: bool) {
        let (caret, goal) = edit::apply(&self.doc, self.caret(), self.goal_col, motion);
        self.cursor.head = caret;
        if !extend {
            self.cursor.anchor = caret;
        }
        self.goal_col = goal;
    }

    /// Replaces the selection, or inserts at the caret.
    pub fn insert_text(&mut self, text: &str) {
        if self.has_selection() {
            let range = self.selection();
            let start = range.start;
            self.doc.remove(range);
            self.cursor = Cursor::caret(start);
        }
        let caret = self.caret();
        self.doc.insert(caret, text);
        self.cursor = Cursor::caret(caret + text.chars().count());
        self.goal_col = None;
        self.refresh();
    }

    /// Deletes the selection or the character before the caret.
    pub fn backspace(&mut self) {
        if self.has_selection() {
            self.insert_text("");
            return;
        }
        let caret = self.caret();
        if caret == 0 {
            return;
        }
        self.doc.remove(caret - 1..caret);
        self.cursor = Cursor::caret(caret - 1);
        self.goal_col = None;
        self.refresh();
    }

    /// Deletes the selection or the character after the caret.
    pub fn delete_forward(&mut self) {
        if self.has_selection() {
            self.insert_text("");
            return;
        }
        let caret = self.caret();
        if caret >= self.doc.len_chars() {
            return;
        }
        self.doc.remove(caret..caret + 1);
        self.cursor = Cursor::caret(caret);
        self.goal_col = None;
        self.refresh();
    }

    pub fn select_all(&mut self) {
        self.cursor = Cursor {
            anchor: 0,
            head: self.doc.len_chars(),
        };
    }

    pub fn selected_text(&self) -> String {
        let range = self.selection();
        self.text
            .chars()
            .skip(range.start)
            .take(range.end - range.start)
            .collect()
    }

    /// Undoes one step and keeps the caret inside the text.
    pub fn undo(&mut self) -> bool {
        if !self.doc.undo() {
            return false;
        }
        self.clamp_caret();
        true
    }

    pub fn redo(&mut self) -> bool {
        if !self.doc.redo() {
            return false;
        }
        self.clamp_caret();
        true
    }

    fn clamp_caret(&mut self) {
        let caret = self.caret().min(self.doc.len_chars());
        self.cursor = Cursor::caret(caret);
        self.refresh();
    }

    /// Rebuilds the flat mirror, the line table and the highlight spans.
    fn refresh(&mut self) {
        self.text = self.doc.text();
        self.line_starts.clear();
        self.line_starts.push(0);
        for (index, byte) in self.text.bytes().enumerate() {
            if byte == b'\n' {
                self.line_starts.push(index + 1);
            }
        }
        if let Some(highlighter) = self.highlighter.as_mut() {
            highlighter.set_text(&self.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> EditorBuffer {
        EditorBuffer::from_text(PathBuf::from("test.rs"), text.to_owned())
    }

    #[test]
    fn line_table_follows_the_text() {
        let buf = buffer("one\ntwo\nthree");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(1), "two");
        assert_eq!(buf.line_starts(), &[0, 4, 8]);
    }

    #[test]
    fn inserting_text_moves_the_caret_and_dirties() {
        let mut buf = buffer("ab");
        buf.set_caret(1);
        buf.insert_text("X");
        assert_eq!(buf.text(), "aXb");
        assert_eq!(buf.caret(), 2);
        assert!(buf.is_dirty());
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut buf = buffer("hello world");
        buf.cursor = Cursor {
            anchor: 6,
            head: 11,
        };
        buf.insert_text("there");
        assert_eq!(buf.text(), "hello there");
        assert_eq!(buf.caret(), 11);
    }

    #[test]
    fn backspace_merges_lines() {
        let mut buf = buffer("one\ntwo");
        buf.set_caret(4);
        buf.backspace();
        assert_eq!(buf.text(), "onetwo");
        assert_eq!(buf.caret(), 3);
    }

    #[test]
    fn delete_forward_at_the_end_is_a_no_op() {
        let mut buf = buffer("abc");
        buf.set_caret(3);
        buf.delete_forward();
        assert_eq!(buf.text(), "abc");
    }

    #[test]
    fn undo_restores_the_text_and_clamps_the_caret() {
        let mut buf = buffer("abc");
        buf.set_caret(3);
        buf.insert_text("def");
        assert!(buf.undo());
        assert_eq!(buf.text(), "abc");
        assert_eq!(buf.caret(), 3);
        assert!(buf.redo());
        assert_eq!(buf.text(), "abcdef");
    }

    #[test]
    fn cyrillic_editing_works_on_char_indices() {
        let mut buf = buffer("привет");
        buf.set_caret(6);
        buf.backspace();
        assert_eq!(buf.text(), "приве");
        buf.insert_text("ё");
        assert_eq!(buf.text(), "привеё");
    }

    #[test]
    fn tabs_and_crlf_do_not_break_the_line_table() {
        let buf = buffer("a\r\n\tb\r\n");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(0), "a");
        assert_eq!(buf.line_text(1), "\tb");
    }

    #[test]
    fn selection_text_is_returned() {
        let mut buf = buffer("abcdef");
        buf.cursor = Cursor { anchor: 1, head: 4 };
        assert_eq!(buf.selected_text(), "bcd");
        assert!(buf.has_selection());
    }

    #[test]
    fn saving_writes_the_file_and_clears_the_dirty_flag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old").unwrap();
        let mut buf = EditorBuffer::load(dir.path(), Path::new("a.txt")).unwrap();
        buf.set_caret(3);
        buf.insert_text("!");
        buf.save(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "old!"
        );
        assert!(!buf.is_dirty());
    }

    #[test]
    fn highlighter_matches_the_language_of_the_path() {
        let buf = buffer("fn main() {}");
        assert!(buf.highlighter.is_some());
        let plain = EditorBuffer::from_text(PathBuf::from("a.unknown"), "text".to_owned());
        assert!(plain.highlighter.is_none());
    }
}
