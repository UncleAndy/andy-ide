//! Renaming a symbol: the word under the caret, and the edits a server answers
//! with, applied from the end so earlier offsets stay valid.
//!
//! The two halves are pure on purpose. A rename touches several files at once, and
//! offsets that are applied in the wrong order produce text that looks almost
//! right — which is the worst kind of damage to a codebase.

/// Characters a name is made of, as far as the caret is concerned.
///
/// Unicode letters and digits are included: renaming `привет` has to work the same
/// way as renaming `hello`.
fn is_name_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The word around `caret`, as a byte range of `text`.
///
/// `caret` may sit at either edge of a character: on a name it ends up inside the
/// name either way, and between two words it picks the one before it.
pub fn word_at(text: &str, caret: usize) -> Option<(usize, usize)> {
    let caret = caret.min(text.len());
    let mut start = caret;
    let mut end = caret;
    // A caret at the right edge of a word belongs to that word.
    if start > 0 && (start == text.len() || !text[start..].starts_with(is_name_char)) {
        let previous = text[..start]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)?;
        if is_name_char(text[previous..].chars().next()?) {
            start = previous;
        }
    }
    while start > 0 {
        let Some((index, ch)) = text[..start].char_indices().next_back() else {
            break;
        };
        if !is_name_char(ch) {
            break;
        }
        start = index;
    }
    while end < text.len() {
        let Some(ch) = text[end..].chars().next() else {
            break;
        };
        if !is_name_char(ch) {
            break;
        }
        end += ch.len_utf8();
    }
    (end > start).then_some((start, end))
}

/// The text with every range replaced by `replacement`.
///
/// The ranges are byte ranges of the same text and may come in any order; they are
/// applied from the end, so a replacement never moves a range that has not been
/// applied yet. Overlapping ranges are refused instead of producing a guess: a
/// server that sends them has a different idea of the file than the editor has.
pub fn apply_edits(text: &str, ranges: &[(usize, usize)], replacement: &str) -> Option<String> {
    let mut sorted: Vec<(usize, usize)> = ranges.to_vec();
    sorted.sort_by_key(|(start, _)| *start);
    let mut previous_end = 0;
    for (start, end) in &sorted {
        if *start < previous_end || start > end || *end > text.len() {
            return None;
        }
        if !text.is_char_boundary(*start) || !text.is_char_boundary(*end) {
            return None;
        }
        previous_end = *end;
    }
    let mut result = text.to_owned();
    for (start, end) in sorted.iter().rev() {
        result.replace_range(*start..*end, replacement);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_under_the_caret_is_the_name() {
        let text = "let value = 1;";
        let caret = text.find("value").unwrap() + 2;
        assert_eq!(word_at(text, caret), Some((4, 9)));
    }

    #[test]
    fn the_caret_at_the_end_of_a_word_still_belongs_to_it() {
        let text = "let value = 1;";
        let caret = text.find("value").unwrap() + "value".len();
        assert_eq!(word_at(text, caret), Some((4, 9)));
    }

    #[test]
    fn outside_a_word_there_is_nothing_to_rename() {
        // Right after a name the caret still belongs to it; with a space on both
        // sides there is no word to take.
        let text = "let  value = 1;";
        assert_eq!(
            word_at(text, 4),
            None,
            "a caret between two spaces was taken for a name"
        );
        assert_eq!(word_at("", 0), None);
    }

    #[test]
    fn cyrillic_names_are_names() {
        let text = "let привет = 1;";
        let start = text.find("привет").unwrap();
        let caret = start + "при".len();
        assert_eq!(word_at(text, caret), Some((start, start + "привет".len())));
    }

    #[test]
    fn edits_are_applied_from_the_end() {
        let text = "a b a";
        // Replacing every `a` with `bb`: the last one first, so the earlier
        // offsets still point at their own `a`.
        assert_eq!(
            apply_edits(text, &[(4, 5), (0, 1)], "bb").as_deref(),
            Some("bb b bb")
        );
    }

    #[test]
    fn edits_given_out_of_order_are_sorted() {
        assert_eq!(
            apply_edits("a b a", &[(0, 1), (4, 5)], "bb").as_deref(),
            Some("bb b bb")
        );
    }

    #[test]
    fn overlapping_edits_are_refused() {
        assert_eq!(apply_edits("abcdef", &[(0, 3), (2, 5)], "x"), None);
    }

    #[test]
    fn edits_behind_the_text_are_refused() {
        assert_eq!(apply_edits("abc", &[(0, 9)], "x"), None);
    }

    #[test]
    fn an_edit_inside_a_character_is_refused() {
        // The `п` of `привет` is two bytes: an offset in the middle of it is a bug
        // in the answer, and slicing there would panic.
        assert_eq!(apply_edits("привет", &[(1, 3)], "x"), None);
    }

    #[test]
    fn a_rename_to_an_empty_name_removes_the_uses() {
        assert_eq!(apply_edits("a b a", &[(0, 1)], "").as_deref(), Some(" b a"));
    }
}
