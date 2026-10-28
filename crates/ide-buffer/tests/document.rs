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
