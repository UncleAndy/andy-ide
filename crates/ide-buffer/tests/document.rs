use ide_buffer::{Cursor, Document};

#[test]
fn insert_and_remove_roundtrip() {
    let mut d = Document::new("hello");
    d.insert(5, " world");
    assert_eq!(d.text(), "hello world");
    d.remove(5..6);
    assert_eq!(d.text(), "helloworld");
}

#[test]
fn index_past_end_is_clamped() {
    let mut d = Document::new("ab");
    d.insert(99, "c");
    assert_eq!(d.text(), "abc");
}

#[test]
fn undo_restores_and_redo_replays() {
    let mut d = Document::new("a");
    d.insert(1, "b");
    assert!(d.can_undo());
    assert!(d.undo());
    assert_eq!(d.text(), "a");
    assert!(d.redo());
    assert_eq!(d.text(), "ab");
}

#[test]
fn consecutive_typing_is_one_undo_step() {
    let mut d = Document::new("");
    for (i, ch) in "abc".chars().enumerate() {
        d.insert(i, &ch.to_string());
    }
    assert!(d.undo());
    assert_eq!(d.text(), "");
}

#[test]
fn line_col_mapping_and_crlf_preserved() {
    let mut d = Document::new("one\r\ntwo\r\n");
    assert_eq!(d.line_count(), 3);
    assert_eq!(d.char_to_line_col(6), (1, 1));
    assert_eq!(d.line_col_to_char(1, 1), 6);
    d.insert(0, "X");
    assert!(d.text().contains("\r\n"));
}

#[test]
fn empty_document_has_one_line() {
    let d = Document::new("");
    assert_eq!(d.line_count(), 1);
    assert_eq!(d.line(0).unwrap().to_string(), "");
}

#[test]
fn cursor_selection_range_is_normalized() {
    let mut c = Cursor::caret(7);
    c.anchor = 3;
    assert_eq!(c.selection(), 3..7);
}

#[test]
fn multibyte_characters_are_indexed_by_char() {
    let mut d = Document::new("привет");
    assert_eq!(d.len_chars(), 6);
    d.insert(6, "!");
    assert_eq!(d.text(), "привет!");
    assert_eq!(d.char_to_line_col(3), (0, 3));
}

#[test]
fn dirty_clears_only_on_mark_saved() {
    let mut d = Document::new("a");
    assert!(!d.is_dirty());
    d.insert(1, "b");
    assert!(d.is_dirty());
    d.mark_saved();
    assert!(!d.is_dirty());
    d.undo();
    assert!(d.is_dirty());
}

#[test]
fn history_is_bounded() {
    let mut d = Document::new("");
    for i in 0..3000 {
        // Each insert lands at a fresh position, so no coalescing happens.
        d.insert(0, &format!("{i} "));
    }
    let mut undone = 0;
    while d.undo() {
        undone += 1;
    }
    assert_eq!(undone, 1000);
}

#[test]
fn line_has_no_terminator() {
    let d = Document::new("one\ntwo\r\nthree");
    assert_eq!(d.line(0).unwrap().to_string(), "one");
    assert_eq!(d.line(1).unwrap().to_string(), "two");
    assert_eq!(d.line(2).unwrap().to_string(), "three");
}

#[test]
fn a_lone_carriage_return_is_a_line_break() {
    // Ropey breaks on `\r`, `\n` and `\r\n` alike; the editor's view of a line
    // must agree with it, otherwise "end of line" lands on another line.
    let doc = Document::new("one\rtwo\r\nthree\nfour");
    assert_eq!(doc.line_count(), 4);
    assert_eq!(doc.line(0).unwrap().to_string(), "one");
    assert_eq!(doc.line(1).unwrap().to_string(), "two");
    assert_eq!(doc.line(2).unwrap().to_string(), "three");
    assert_eq!(doc.line(3).unwrap().to_string(), "four");
    assert_eq!(doc.line_col_to_char(1, 0), 4);
    assert_eq!(doc.char_to_line_col(6), (1, 2));
}

#[test]
fn replacing_a_range_is_one_undo_step() {
    let mut d = Document::new("one two one");
    d.replace_range(0..3, "1");
    // The second replacement is written where the text stands now, not where it stood before
    // the first one: the first made the document two characters shorter.
    d.replace_range(6..9, "1");
    assert_eq!(d.text(), "1 two 1");
    // Each replacement is its own step, and each reverts on its own.
    assert!(d.undo());
    assert_eq!(d.text(), "1 two one");
    assert!(d.undo());
    assert_eq!(d.text(), "one two one");
    // And forward again, in the same order.
    assert!(d.redo());
    assert_eq!(d.text(), "1 two one");
    assert!(d.redo());
    assert_eq!(d.text(), "1 two 1");
}

#[test]
fn a_replacement_that_takes_nothing_away_extends_a_typing_step() {
    // An insertion at the caret is a replacement of nothing, and behaves as typing does.
    let mut d = Document::new("ab");
    d.replace_range(2..2, "c");
    assert_eq!(d.text(), "abc");
    assert!(d.undo());
    assert_eq!(d.text(), "ab");
}

#[test]
fn a_replacement_of_a_stretch_that_is_longer_than_one_character_reverts_whole() {
    let mut d = Document::new("before middle after");
    d.replace_range(7..13, "X");
    assert_eq!(d.text(), "before X after");
    assert!(d.undo());
    assert_eq!(d.text(), "before middle after");
}
