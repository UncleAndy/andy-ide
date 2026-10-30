//! Finding text in a document: the needle, the matches and the replacement.
//!
//! Ranges here are byte offsets into the whole text, like every other range the window's
//! own text carries (the highlighter and the renderer work in bytes); the caller that
//! counts characters — the caret of a buffer, for one — converts where it stands.
//!
//! No regular expressions: the spec asks for find and replace in a file, and a needle that
//! is a plain string is the whole of it. Case is folded one character at a time and never
//! one string at a time: the lowercase of `İ` is two characters long, and a mapping that
//! changes lengths would move every offset after it.

use std::ops::Range;

/// One match: a byte range in the text.
pub type Hit = Range<usize>;

/// What to look for, and how strictly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Needle {
    /// The text to find.
    pub text: String,
    /// Whether the cases have to agree.
    pub case_sensitive: bool,
    /// Whether the match has to begin and end at the edge of a word.
    pub whole_word: bool,
}

impl Needle {
    /// A needle that ignores case and word edges, as the search bar opens.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            case_sensitive: false,
            whole_word: false,
        }
    }

    /// A needle with nothing to look for draws no matches at all.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Every match, from left to right, none of them overlapping.
pub fn all(text: &str, needle: &Needle) -> Vec<Hit> {
    let mut hits = Vec::new();
    let bytes = needle.text.len().max(1);
    let mut from = 0;
    while from <= text.len() {
        let Some(hit) = next(text, needle, from) else {
            break;
        };
        // Past this match, and never on the same spot again: a needle of one
        // character matches at every step, and stepping by a match's own start
        // would find it for ever.
        from = hit.end.max(hit.start + bytes).max(hit.start + 1);
        hits.push(hit);
    }
    hits
}

/// The first match that begins at or after `from`.
pub fn next(text: &str, needle: &Needle, from: usize) -> Option<Hit> {
    if needle.is_empty() {
        return None;
    }
    let mut at = boundary(text, from.min(text.len()));
    while at <= text.len() {
        if let Some(end) = matches_at(text, at, needle)
            && edges_are_words(text, at, end, needle)
        {
            return Some(at..end);
        }
        at = match text[at..].chars().next() {
            Some(c) => at + c.len_utf8(),
            None => break,
        };
    }
    None
}

/// The last match that begins before `from` — the one behind the caret.
pub fn previous(text: &str, needle: &Needle, from: usize) -> Option<Hit> {
    if needle.is_empty() {
        return None;
    }
    let before = boundary(text, from.min(text.len()));
    let mut found = None;
    let mut at = 0;
    while at < before {
        if let Some(end) = matches_at(text, at, needle)
            && edges_are_words(text, at, end, needle)
        {
            found = Some(at..end);
        }
        at = match text[at..].chars().next() {
            Some(c) => at + c.len_utf8(),
            None => break,
        };
    }
    found
}

/// Replaces every match. Returns the new text and how many places were replaced.
pub fn replace_all(text: &str, needle: &Needle, replacement: &str) -> (String, usize) {
    let hits = all(text, needle);
    if hits.is_empty() {
        return (text.to_owned(), 0);
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for hit in &hits {
        out.push_str(&text[at..hit.start]);
        out.push_str(replacement);
        at = hit.end;
    }
    out.push_str(&text[at..]);
    (out, hits.len())
}

/// The end of a match that begins at `at`, if the needle is there.
///
/// `at` must stand on a character boundary.
fn matches_at(text: &str, at: usize, needle: &Needle) -> Option<usize> {
    let rest = &text[at..];
    let mut hay = rest.chars();
    for wanted in needle.text.chars() {
        let found = hay.next()?;
        if !same(found, wanted, needle.case_sensitive) {
            return None;
        }
    }
    // What the iterator has not consumed is what the match left over.
    Some(at + (rest.len() - hay.as_str().len()))
}

/// Whether two characters are the same letter, when case is not held against them.
fn same(found: char, wanted: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        return found == wanted;
    }
    fold(found) == fold(wanted)
}

/// One character, folded to one character.
///
/// `İ` folds to two characters and is left as it is: a fold that lengthens a character
/// would not line up with the text it is compared against.
fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

/// Whether a match begins and ends at the edge of a word, when that is asked for.
fn edges_are_words(text: &str, start: usize, end: usize, needle: &Needle) -> bool {
    if !needle.whole_word {
        return true;
    }
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(is_word) && !after.is_some_and(is_word)
}

/// What a word is made of: a letter, a digit, or an underscore, as everywhere else.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The nearest character boundary at or before `at`.
fn boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(text: &str, needle: &str) -> Vec<(usize, usize)> {
        all(text, &Needle::new(needle))
            .into_iter()
            .map(|hit| (hit.start, hit.end))
            .collect()
    }

    #[test]
    fn every_match_is_found_left_to_right() {
        assert_eq!(hits("one two one", "one"), vec![(0, 3), (8, 11)]);
    }

    #[test]
    fn case_is_ignored_until_it_is_asked_for() {
        assert_eq!(hits("Value value VALUE", "value").len(), 3);
        let strict = Needle {
            text: "value".to_owned(),
            case_sensitive: true,
            whole_word: false,
        };
        assert_eq!(all("Value value VALUE", &strict).len(), 1);
        // And the letters of another alphabet fold as well.
        assert_eq!(hits("Ёлка ёЛКА", "ёлка").len(), 2);
    }

    #[test]
    fn a_fold_that_would_lengthen_a_character_is_left_alone() {
        // `İ` lowercases to `i` and a combining dot: two characters where there was one.
        // The offset after it must not move, so the character stands as it is.
        assert_eq!(hits("İstanbul", "istanbul").len(), 0);
        assert_eq!(hits("İstanbul", "İstanbul").len(), 1);
    }

    #[test]
    fn a_whole_word_does_not_answer_from_inside_another() {
        let word = Needle {
            text: "cat".to_owned(),
            case_sensitive: false,
            whole_word: true,
        };
        assert!(all("concatenate scatter", &word).is_empty());
        assert_eq!(all("a cat, a cat's", &word).len(), 2);
    }

    #[test]
    fn the_matches_go_forwards_and_backwards_from_a_place() {
        let text = "one two one two one";
        let needle = Needle::new("one");
        assert_eq!(next(text, &needle, 0), Some(0..3));
        assert_eq!(next(text, &needle, 1), Some(8..11));
        assert_eq!(next(text, &needle, 9), Some(16..19));
        assert_eq!(next(text, &needle, 17), None);
        assert_eq!(previous(text, &needle, 17), Some(16..19));
        assert_eq!(previous(text, &needle, 9), Some(8..11));
        assert_eq!(previous(text, &needle, 0), None);
    }

    #[test]
    fn a_needle_with_nothing_in_it_finds_nothing() {
        assert!(all("some text", &Needle::new("")).is_empty());
        assert_eq!(next("some text", &Needle::default(), 0), None);
        assert_eq!(replace_all("some text", &Needle::new(""), "-").1, 0);
    }

    #[test]
    fn a_match_may_span_lines_and_may_hold_a_character_of_several_bytes() {
        assert_eq!(hits("first\nsecond\nfirst", "first\nsecond"), vec![(0, 12)]);
        // Offsets are bytes: «ёж» is four bytes, and a space is one while «и» is two.
        let text = "ёж и ёж";
        let found = hits(text, "ёж");
        assert_eq!(found, vec![(0, 4), (8, 12)]);
        assert_eq!(&text[8..12], "ёж");
    }

    #[test]
    fn replacing_takes_every_match_and_counts_them() {
        let text = "one two one";
        let (out, count) = replace_all(text, &Needle::new("one"), "1");
        assert_eq!((out.as_str(), count), ("1 two 1", 2));
        // A replacement of nothing takes the matches away.
        let (out, count) = replace_all(text, &Needle::new("one "), "");
        assert_eq!((out.as_str(), count), ("two one", 1));
        // The replacement is put in as it is written: no needle is looked at inside it.
        let (out, _) = replace_all("one", &Needle::new("one"), "one one");
        assert_eq!(out, "one one");
    }
}
