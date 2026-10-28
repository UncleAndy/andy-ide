//! The text editor: one buffer per open tab.

pub mod edit;
pub mod view;

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ide_buffer::{Cursor, Document};
use ide_syntax::{Highlighter, Language};

/// Spaces per indent level; also how wide a tab is drawn.
pub const INDENT_WIDTH: usize = 4;

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
    /// Byte offset just past the last character of each line, terminators excluded.
    line_ends: Vec<usize>,
    /// Widest line in characters, cached: the horizontal extent needs it every
    /// frame, and counting the whole document per frame is what used to make
    /// scrolling through a big file sluggish.
    max_line_chars: usize,
    /// A Ctrl+Click asked for the definition of the character at this index, and
    /// the application has not answered it yet.
    nav_request: Option<usize>,
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
        // The text reaches the highlighter through `refresh`, which knows how to
        // hand it over incrementally.
        let highlighter = language.map(Highlighter::new);

        let mut buffer = Self {
            path,
            doc: Document::new(&text),
            cursor: Cursor::caret(0),
            highlighter,
            goal_col: None,
            text: String::new(),
            line_starts: Vec::new(),
            line_ends: Vec::new(),
            max_line_chars: 0,
            nav_request: None,
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

    /// Replaces the whole text with what is on disk now.
    ///
    /// Used when a file changed behind the editor and the buffer has no edits
    /// of its own. The caret stays where it was, clamped to the new text, and
    /// the buffer is clean afterwards.
    pub fn replace_text(&mut self, text: String) {
        let character = self.caret().min(text.chars().count());
        self.doc = Document::new(&text);
        self.doc.mark_saved();
        self.cursor = Cursor::caret(character);
        self.goal_col = None;
        self.refresh();
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

    /// One line without its terminator (`\n`, `\r\n` or a lone `\r`).
    pub fn line_text(&self, line: usize) -> &str {
        let (Some(&start), Some(&end)) = (self.line_starts.get(line), self.line_ends.get(line))
        else {
            return "";
        };
        let end = end.clamp(start, self.text.len());
        &self.text[start..end]
    }

    /// Width of the longest line in characters, for the horizontal extent.
    pub fn max_line_chars(&self) -> usize {
        self.max_line_chars
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

    /// Records that the character at this index was clicked with Ctrl held.
    pub fn request_navigation(&mut self, index: usize) {
        self.nav_request = Some(index.min(self.doc.len_chars()));
    }

    /// Takes the pending navigation request, if there is one.
    pub fn take_nav_request(&mut self) -> Option<usize> {
        self.nav_request.take()
    }

    /// The caret as a byte offset, for anything that speaks bytes.
    pub fn caret_byte(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.caret())
            .map(|(index, _)| index)
            .unwrap_or(self.text.len())
    }

    /// Moves the caret to a byte offset, snapping to a character boundary.
    pub fn set_caret_byte(&mut self, byte: usize) {
        let mut byte = byte.min(self.text.len());
        while byte > 0 && !self.text.is_char_boundary(byte) {
            byte -= 1;
        }
        self.set_caret(self.text[..byte].chars().count());
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

    /// Indents the selected lines, or inserts one indent at the caret.
    ///
    /// The selection is widened to whole lines first, the way other editors do
    /// it, so a selection that stops in the middle of a line still indents that
    /// line completely.
    pub fn indent(&mut self) {
        self.change_indent(false);
    }

    /// Removes one indent level from the selected lines.
    pub fn outdent(&mut self) {
        self.change_indent(true);
    }

    fn change_indent(&mut self, outdent: bool) {
        let (first, last) = self.selected_lines();

        if first == last && !self.has_selection() {
            if outdent {
                self.outdent_current_line(first);
            } else {
                self.insert_text(&" ".repeat(INDENT_WIDTH));
            }
            return;
        }

        let reversed = self.cursor.head < self.cursor.anchor;

        // From the last line upwards, so earlier edits do not shift later lines.
        for line in (first..=last).rev() {
            let start = self.doc.line_col_to_char(line, 0);
            if outdent {
                self.remove_indent_at(start);
            } else {
                self.doc.insert(start, &" ".repeat(INDENT_WIDTH));
            }
        }

        // The whole lines were re-indented, so the whole lines stay selected.
        // `line_text` reads the cached copy, which is still stale here.
        self.refresh();
        let start = self.doc.line_col_to_char(first, 0);
        let end_column = self.line_text(last).chars().count();
        let end = self.doc.line_col_to_char(last, end_column);
        self.cursor = if reversed {
            Cursor {
                anchor: end,
                head: start,
            }
        } else {
            Cursor {
                anchor: start,
                head: end,
            }
        };
        self.goal_col = None;
        self.refresh();
    }

    /// Removes up to one indent worth of leading whitespace at `start`.
    fn remove_indent_at(&mut self, start: usize) -> usize {
        let mut removed = 0;
        let end = (start + INDENT_WIDTH).min(self.doc.len_chars());
        let mut index = start;
        while index < end {
            let ch = self.char_at_view(index);
            if ch == ' ' || ch == '\t' {
                index += 1;
                removed += 1;
            } else {
                break;
            }
        }
        if removed > 0 {
            self.doc.remove(start..start + removed);
        }
        removed
    }

    fn outdent_current_line(&mut self, line: usize) {
        let start = self.doc.line_col_to_char(line, 0);
        let caret = self.caret();
        let removed = self.remove_indent_at(start);
        let caret = caret.saturating_sub(removed.min(caret.saturating_sub(start)));
        self.cursor = Cursor::caret(caret);
        self.goal_col = None;
        self.refresh();
    }

    /// Line numbers of the selection, widened to whole lines.
    fn selected_lines(&self) -> (usize, usize) {
        let range = self.selection();
        let (first, _) = self.doc.char_to_line_col(range.start);
        let (last, last_col) = self.doc.char_to_line_col(range.end);
        // A selection that ends at column 0 does not touch the last line.
        let last = if last_col == 0 && last > first {
            last - 1
        } else {
            last
        };
        (first, last)
    }

    fn char_at_view(&self, index: usize) -> char {
        self.text.chars().nth(index).unwrap_or('\0')
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
        let previous = std::mem::take(&mut self.text);
        self.text = self.doc.text();
        let lines = split_lines(&self.text);
        self.max_line_chars = lines.iter().map(|line| line.width).max().unwrap_or(0);
        self.line_starts = lines.iter().map(|line| line.start).collect();
        self.line_ends = lines.iter().map(|line| line.end).collect();

        // Handing one edit to the highlighter lets tree-sitter reparse just the
        // changed region; replacing the whole text would reparse the file on
        // every keystroke.
        if let Some(highlighter) = self.highlighter.as_mut()
            && let Some((start, old_end, new_end)) = common_edit(&previous, &self.text)
        {
            highlighter.edit(start, old_end, new_end, &self.text[start..new_end]);
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

#[cfg(test)]
mod indent_tests {
    use super::*;

    fn buffer(text: &str) -> EditorBuffer {
        EditorBuffer::from_text(PathBuf::from("a.rs"), text.to_owned())
    }

    #[test]
    fn tab_at_the_caret_inserts_one_indent() {
        let mut buf = buffer("abc");
        buf.set_caret(1);
        buf.indent();
        assert_eq!(buf.text(), "a    bc");
        assert_eq!(buf.caret(), 5);
    }

    #[test]
    fn tab_indents_every_selected_line() {
        let mut buf = buffer("one\ntwo\nthree");
        // From the middle of the first line to the middle of the second.
        buf.cursor = Cursor { anchor: 1, head: 6 };
        buf.indent();
        assert_eq!(buf.text(), "    one\n    two\nthree");
        assert_eq!(buf.selected_text(), "    one\n    two");
    }

    #[test]
    fn shift_tab_removes_one_level_from_each_line() {
        let mut buf = buffer("    one\n        two\nthree");
        buf.cursor = Cursor {
            anchor: 0,
            head: buf.doc.len_chars(),
        };
        buf.outdent();
        assert_eq!(buf.text(), "one\n    two\nthree");
    }

    #[test]
    fn shift_tab_stops_at_the_text_and_survives_a_fresh_line() {
        let mut buf = buffer("  x\n\n");
        buf.cursor = Cursor {
            anchor: 0,
            head: buf.doc.len_chars(),
        };
        buf.outdent();
        assert_eq!(buf.text(), "x\n\n");
    }

    #[test]
    fn shift_tab_at_the_caret_only_touches_its_own_line() {
        let mut buf = buffer("    keep\n    change\n");
        buf.set_caret(0);
        buf.outdent();
        assert_eq!(buf.text(), "keep\n    change\n");
        assert_eq!(buf.caret(), 0);
    }

    #[test]
    fn a_tab_character_counts_as_one_indent() {
        let mut buf = buffer("\tx\n");
        buf.set_caret(1);
        buf.outdent();
        assert_eq!(buf.text(), "x\n");
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::{common_edit, split_lines};
    use ide_buffer::Document;

    #[test]
    fn the_line_table_matches_the_rope() {
        for text in [
            "",
            "a",
            "a\n",
            "a\nb",
            "a\r\nb",
            "a\rb",
            "\r\n",
            "\r",
            "a\r\n\r\nb\n",
            "первая\nвторая\r\nтретья\r",
        ] {
            let doc = Document::new(text);
            let lines = split_lines(text);
            assert_eq!(lines.len(), doc.line_count(), "line count of {text:?}");
            for (index, line) in lines.iter().enumerate() {
                assert_eq!(
                    line.start,
                    doc.line_start_byte(index),
                    "start of {index} in {text:?}"
                );
                assert_eq!(
                    &text[line.start..line.end],
                    doc.line(index).unwrap().to_string(),
                    "text of line {index} in {text:?}"
                );
                assert_eq!(
                    line.width,
                    text[line.start..line.end].chars().count(),
                    "width of line {index} in {text:?}"
                );
            }
        }
    }

    #[test]
    fn widths_count_characters_not_bytes() {
        let lines = split_lines("привет\nмир\n");
        assert_eq!(lines[0].width, 6);
        assert_eq!(lines[1].width, 3);
    }

    #[test]
    fn an_edit_is_found_in_the_middle_of_the_text() {
        assert_eq!(common_edit("abcd", "abcd"), None);
        assert_eq!(common_edit("abcd", "abXd"), Some((2, 3, 3)));
        assert_eq!(common_edit("abcd", "abcdX"), Some((4, 4, 5)));
        assert_eq!(common_edit("abcd", "abc"), Some((3, 4, 3)));
    }

    #[test]
    fn an_edit_never_splits_a_character() {
        // The byte in the middle of a two-byte letter must not become a boundary.
        let (start, old_end, new_end) = common_edit("привет", "привет!").unwrap();
        assert_eq!((start, old_end), (12, 12));
        assert_eq!(new_end, 13);
        let (start, _, new_end) = common_edit("мама", "папа").unwrap();
        assert!("мама".is_char_boundary(start) && "папа".is_char_boundary(new_end));
    }
}

/// One line of the mirror text, terminators excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MirrorLine {
    start: usize,
    end: usize,
    /// Characters in the line, without the terminator.
    width: usize,
}

/// Splits text the way ropey does: `\n`, `\r\n` and a lone `\r` all end a line.
///
/// One pass over the bytes: asking the rope for every line start costs a tree
/// descent per line, which shows up on large files.
fn split_lines(text: &str) -> Vec<MirrorLine> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let skip = match bytes[index] {
            b'\n' => 1,
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            b'\r' => 1,
            _ => 0,
        };
        if skip == 0 {
            index += 1;
            continue;
        }
        lines.push(MirrorLine {
            start,
            end: index,
            width: char_count(&bytes[start..index]),
        });
        index += skip;
        start = index;
    }
    lines.push(MirrorLine {
        start,
        end: bytes.len(),
        width: char_count(&bytes[start..]),
    });
    lines
}

/// Counts characters in UTF-8 bytes without decoding the text.
fn char_count(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .filter(|byte| (**byte & 0b1100_0000) != 0b1000_0000)
        .count()
}

/// The one replacement that turns `old` into `new`, as `(start, old_end, new_end)`.
///
/// `None` when the texts are equal. The boundaries never split a character, so
/// the range can be handed to tree-sitter as it is.
fn common_edit(old: &str, new: &str) -> Option<(usize, usize, usize)> {
    if old == new {
        return None;
    }
    let old_bytes = old.as_bytes();
    let new_bytes = new.as_bytes();

    let limit = old_bytes.len().min(new_bytes.len());
    let mut prefix = 0;
    while prefix < limit && old_bytes[prefix] == new_bytes[prefix] {
        prefix += 1;
    }
    while prefix > 0 && (!old.is_char_boundary(prefix) || !new.is_char_boundary(prefix)) {
        prefix -= 1;
    }

    let mut suffix = 0;
    let limit = limit - prefix;
    while suffix < limit
        && old_bytes[old_bytes.len() - 1 - suffix] == new_bytes[new_bytes.len() - 1 - suffix]
    {
        suffix += 1;
    }
    while suffix > 0
        && (!old.is_char_boundary(old_bytes.len() - suffix)
            || !new.is_char_boundary(new_bytes.len() - suffix))
    {
        suffix -= 1;
    }

    Some((prefix, old_bytes.len() - suffix, new_bytes.len() - suffix))
}
