//! Caret motions and word helpers.
//!
//! Everything here is a pure function of the text and the caret index, so the
//! editor's behaviour can be tested without a window.

use ide_buffer::Document;

/// A caret movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    Home,
    End,
    DocStart,
    DocEnd,
    PageUp(usize),
    PageDown(usize),
}

/// Where the caret goes and which visual column it aims at.
///
/// `goal` is the column remembered across vertical moves: moving down through a
/// short line and then into a long one returns to the original column, as in
/// every other editor.
pub fn apply(
    doc: &Document,
    caret: usize,
    goal: Option<usize>,
    motion: Motion,
) -> (usize, Option<usize>) {
    let len = doc.len_chars();
    let caret = caret.min(len);
    let (line, col) = doc.char_to_line_col(caret);

    match motion {
        Motion::Left => (prev_char(doc, caret), None),
        Motion::Right => (next_char(doc, caret), None),
        Motion::Up => vertical(doc, line, col, goal, -1),
        Motion::Down => vertical(doc, line, col, goal, 1),
        Motion::WordLeft => (word_left(doc, caret), None),
        Motion::WordRight => (word_right(doc, caret), None),
        Motion::Home => (home_stop(doc, line, col), None),
        Motion::End => (doc.line_col_to_char(line, line_len(doc, line)), None),
        Motion::DocStart => (0, None),
        Motion::DocEnd => (len, None),
        Motion::PageUp(rows) => vertical(doc, line, col, goal, -(rows as isize)),
        Motion::PageDown(rows) => vertical(doc, line, col, goal, rows as isize),
    }
}

/// Where the Home key stops on this line.
///
/// Two stops on the line, taking turns — the first letter of the text on that line, and the line's own edge.
/// Andy: "если курсор дальше начала текста в строке — на начало текста; если он в начале текста или левее — в
/// начало строки; если в начале строки — на начало текста В ТОЙ-ЖЕ СТРОКЕ". The start of the text there means
/// the start of the text on that line, not the start of the file: reaching the file is what Ctrl+Home is for.
fn home_stop(doc: &Document, line: usize, col: usize) -> usize {
    let Some(text) = doc.line(line) else {
        return doc.line_col_to_char(line, 0);
    };
    // The first letter: the first column that is neither a space nor a tab. A line of nothing but spaces has
    // no first letter of its own, and then both stops fall on its edge.
    let first = text
        .chars()
        .position(|ch| ch != ' ' && ch != '\t')
        .unwrap_or(0)
        .min(line_len(doc, line));
    if col > first {
        doc.line_col_to_char(line, first)
    } else if col > 0 {
        doc.line_col_to_char(line, 0)
    } else {
        doc.line_col_to_char(line, first)
    }
}

fn vertical(
    doc: &Document,
    line: usize,
    col: usize,
    goal: Option<usize>,
    delta: isize,
) -> (usize, Option<usize>) {
    let target = goal.unwrap_or(col);
    let lines = doc.line_count();
    let new_line = (line as isize + delta).clamp(0, lines as isize - 1) as usize;
    if new_line == line {
        return (
            doc.line_col_to_char(line, col.min(line_len(doc, line))),
            Some(target),
        );
    }
    let column = target.min(line_len(doc, new_line));
    (doc.line_col_to_char(new_line, column), Some(target))
}

fn line_len(doc: &Document, line: usize) -> usize {
    doc.line(line).map(|slice| slice.len_chars()).unwrap_or(0)
}

/// Characters that belong to a word: letters, digits and the underscore.
pub fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn prev_char(_doc: &Document, caret: usize) -> usize {
    // ropey indexes by Unicode scalar values, so one step is one character.
    caret.saturating_sub(1)
}

fn next_char(doc: &Document, caret: usize) -> usize {
    let len = doc.len_chars();
    if caret >= len {
        return len;
    }
    caret + 1
}

fn char_at(doc: &Document, index: usize) -> char {
    if index >= doc.len_chars() {
        return '\0';
    }
    let (line, _) = doc.char_to_line_col(index);
    let line_start = doc.line_col_to_char(line, 0);
    doc.line(line)
        .and_then(|slice| slice.chars().nth(index - line_start))
        .unwrap_or('\0')
}

fn word_left(doc: &Document, caret: usize) -> usize {
    let mut index = caret;
    while index > 0 {
        let previous = prev_char(doc, index);
        if !is_word_char(char_at(doc, previous)) {
            index = previous;
        } else {
            break;
        }
    }
    while index > 0 {
        let previous = prev_char(doc, index);
        if is_word_char(char_at(doc, previous)) {
            index = previous;
        } else {
            break;
        }
    }
    index
}

fn word_right(doc: &Document, caret: usize) -> usize {
    let len = doc.len_chars();
    let mut index = caret;
    if is_word_char(char_at(doc, index)) {
        // Inside a word: stop at its end, the way other editors do.
        while index < len && is_word_char(char_at(doc, index)) {
            index += 1;
        }
    } else {
        while index < len && !is_word_char(char_at(doc, index)) {
            index += 1;
        }
    }
    index
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_home_key_stops_three_times() {
        let d = Document::new("    let x = 1;\nnext\n");
        // Past the first letter: to the first letter of the line.
        assert_eq!(
            apply(&d, d.line_col_to_char(0, 10), None, Motion::Home).0,
            d.line_col_to_char(0, 4)
        );
        // At the first letter: to the line's edge.
        assert_eq!(apply(&d, d.line_col_to_char(0, 4), None, Motion::Home).0, 0);
        // Left of it, inside the indent: to the line's edge as well.
        assert_eq!(apply(&d, d.line_col_to_char(0, 2), None, Motion::Home).0, 0);
        // At the edge: back to the first letter of that same line — Andy: "если курсор стоит в начале строки и
        // нажимается Home, курсор должен переместиться в начало текста В ТОЙ-ЖЕ СТРОКЕ".
        assert_eq!(apply(&d, 0, None, Motion::Home).0, d.line_col_to_char(0, 4));
        // And so the two stops take turns, without ever leaving the line.
        assert_eq!(apply(&d, d.line_col_to_char(0, 4), None, Motion::Home).0, 0);
        assert_eq!(apply(&d, 0, None, Motion::Home).0, d.line_col_to_char(0, 4));
        // A line whose first letter is its edge has nowhere to take turns to, and the caret stays.
        assert_eq!(
            apply(&d, d.line_col_to_char(1, 2), None, Motion::Home).0,
            d.line_col_to_char(1, 0),
            "a line whose first letter is its edge stopped at that edge"
        );
        assert_eq!(
            apply(&d, d.line_col_to_char(1, 0), None, Motion::Home).0,
            d.line_col_to_char(1, 0),
            "the caret left a line that has nowhere to turn to"
        );
    }

    use super::*;

    fn doc(text: &str) -> Document {
        Document::new(text)
    }

    #[test]
    fn left_and_right_stop_at_the_edges() {
        let d = doc("abc");
        assert_eq!(apply(&d, 0, None, Motion::Left).0, 0);
        assert_eq!(apply(&d, 3, None, Motion::Right).0, 3);
        assert_eq!(apply(&d, 1, None, Motion::Right).0, 2);
    }

    #[test]
    fn down_keeps_the_goal_column() {
        let d = doc("long line\nx\nlong again\n");
        // Column 7 on line 0 clamps to 1 on line 1 but the goal is remembered.
        let (caret, goal) = apply(&d, 7, None, Motion::Down);
        assert_eq!(caret, d.line_col_to_char(1, 1));
        assert_eq!(goal, Some(7));
        let (caret, _) = apply(&d, caret, goal, Motion::Down);
        assert_eq!(caret, d.line_col_to_char(2, 7));
    }

    #[test]
    fn up_at_the_first_line_stays() {
        let d = doc("first\nsecond\n");
        assert_eq!(apply(&d, 2, None, Motion::Up).0, 2);
    }

    #[test]
    fn home_and_end_work_on_the_current_line() {
        let d = doc("alpha\nbeta\n");
        let midline = d.line_col_to_char(1, 2);
        assert_eq!(
            apply(&d, midline, None, Motion::Home).0,
            d.line_col_to_char(1, 0)
        );
        assert_eq!(
            apply(&d, midline, None, Motion::End).0,
            d.line_col_to_char(1, 4)
        );
    }

    #[test]
    fn word_motion_skips_separators() {
        let d = doc("let value = compute(1);");
        let start = 0;
        let after_let = apply(&d, start, None, Motion::WordRight).0;
        assert_eq!(d.char_to_line_col(after_let), (0, 3));
        let word_start = apply(&d, after_let, None, Motion::WordRight).0;
        assert_eq!(d.char_to_line_col(word_start).1, 4);
        let after_value = apply(&d, word_start, None, Motion::WordRight).0;
        assert_eq!(d.char_to_line_col(after_value).1, 9);
        let back = apply(&d, after_value, None, Motion::WordLeft).0;
        assert_eq!(d.char_to_line_col(back).1, 4);
    }

    #[test]
    fn cyrillic_counts_as_one_word() {
        let d = doc("пусть значение = 1");
        let after_first = apply(&d, 0, None, Motion::WordRight).0;
        assert_eq!(d.char_to_line_col(after_first).1, 5);
    }

    #[test]
    fn page_down_is_bounded_by_the_last_line() {
        let d = doc("a\nb\nc\nd\ne\n");
        let (caret, _) = apply(&d, 0, None, Motion::PageDown(40));
        assert_eq!(d.char_to_line_col(caret).0, d.line_count() - 1);
    }

    #[test]
    fn document_start_and_end() {
        let d = doc("one\ntwo");
        assert_eq!(apply(&d, 3, None, Motion::DocStart).0, 0);
        assert_eq!(apply(&d, 3, None, Motion::DocEnd).0, d.len_chars());
    }
}
