//! Links in what a program printed: a URL, or a file with a line.
//!
//! The screen only ever holds characters, so a link is whatever the text says. The
//! parsing lives here, away from the window, because it has rules of its own: the
//! trailing full stop of a sentence is not part of a URL, and `3.4` is not a file
//! called `3`.

use std::ops::Range;

/// What clicking a link should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// A page to open in the browser.
    Url(String),
    /// A file to open in the editor, with the line the message named.
    File {
        path: String,
        /// One-based line, as compilers print it.
        line: u32,
        /// One-based column, when the message gave one.
        column: u32,
    },
}

/// The link under a column of a line, and the columns it covers.
///
/// `column` counts characters, as the terminal grid does.
pub fn find(line: &str, column: usize) -> Option<(Range<usize>, Link)> {
    let characters: Vec<char> = line.chars().collect();
    if column >= characters.len() {
        return None;
    }
    let (start, end) = word_at(&characters, column)?;
    let token: String = characters[start..end].iter().collect();
    let link = parse(&token)?;
    Some((start..end, link))
}

/// The word the column falls in: everything up to a space or a quote.
fn word_at(characters: &[char], column: usize) -> Option<(usize, usize)> {
    if !is_word(characters[column]) {
        return None;
    }
    let mut start = column;
    while start > 0 && is_word(characters[start - 1]) {
        start -= 1;
    }
    let mut end = column + 1;
    while end < characters.len() && is_word(characters[end]) {
        end += 1;
    }
    // A sentence ends with a full stop and a list with a comma; neither belongs to
    // the link. Brackets do when they were opened inside the word, as in `[1]`.
    while end > start + 1 {
        let last = characters[end - 1];
        let trimmed = matches!(last, '.' | ',' | ';' | ':' | '!' | '?')
            || (matches!(last, ')' | ']' | '}') && !opened_inside(&characters[start..end], last));
        if trimmed {
            end -= 1;
        } else {
            break;
        }
    }
    while start < end && matches!(characters[start], '(' | '[' | '{') {
        start += 1;
    }
    (start < end).then_some((start, end))
}

/// Whether the closing bracket has its opening bracket inside the word, which is
/// how `(https://example.com)` keeps its bracket out and `wiki/Foo_(bar)` keeps its.
fn opened_inside(characters: &[char], closing: char) -> bool {
    let opening = match closing {
        ')' => '(',
        ']' => '[',
        _ => '{',
    };
    characters.contains(&opening)
}

fn is_word(character: char) -> bool {
    !character.is_whitespace() && !matches!(character, '"' | '\'' | '<' | '>' | '`' | '«' | '»')
}

/// Reads one word as a link.
pub fn parse(token: &str) -> Option<Link> {
    if token.starts_with("http://") || token.starts_with("https://") {
        // A URL needs a host, not just a scheme.
        let rest = token.split_once("://").map(|(_, rest)| rest)?;
        if rest.is_empty() || rest.starts_with('/') {
            return None;
        }
        return Some(Link::Url(token.to_owned()));
    }
    // `file.rs:12` and `file.rs:12:5`; the path must look like a path, so that a
    // version number or a time of day is not mistaken for one.
    let (before, last) = token.rsplit_once(':')?;
    let (path, line_text, column_text) = match before.rsplit_once(':') {
        Some((path, line)) if line.chars().all(|c| c.is_ascii_digit()) && !line.is_empty() => {
            (path, line, Some(last))
        }
        _ => (before, last, None),
    };
    if line_text.is_empty() || !line_text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let line: u32 = line_text.parse().ok()?;
    let column: u32 = match column_text {
        Some(text) => text.parse().ok()?,
        None => 1,
    };
    if !looks_like_a_path(path) {
        return None;
    }
    Some(Link::File {
        path: path.to_owned(),
        line,
        column,
    })
}

/// A path has a separator or an extension; a bare word does not.
fn looks_like_a_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with(' ') {
        return false;
    }
    let has_separator = path.contains('/') || path.contains('\\');
    let has_extension = path
        .rsplit_once('.')
        .is_some_and(|(name, extension)| !name.is_empty() && extension.len() <= 5);
    (has_separator || has_extension) && !path.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::{Link, find, parse};

    fn file(path: &str, line: u32) -> Link {
        Link::File {
            path: path.to_owned(),
            line,
            column: 1,
        }
    }

    #[test]
    fn a_url_is_found_under_the_pointer() {
        let line = "see https://example.com/a?b=1 for more";
        let (range, link) = find(line, 10).expect("a link");
        assert_eq!(
            link,
            Link::Url("https://example.com/a?b=1".to_owned()),
            "the link is not the whole URL"
        );
        assert_eq!(&line[range.clone()], "https://example.com/a?b=1");
    }

    #[test]
    fn the_full_stop_of_a_sentence_is_not_part_of_it() {
        let line = "See https://example.com/page.";
        let (range, link) = find(line, 8).expect("a link");
        assert_eq!(range.len(), "https://example.com/page".len());
        assert_eq!(link, Link::Url("https://example.com/page".to_owned()));
    }

    #[test]
    fn a_bracket_around_a_url_belongs_to_the_sentence() {
        let (_, link) = find("(see https://example.com/x)", 12).expect("a link");
        assert_eq!(link, Link::Url("https://example.com/x".to_owned()));
        // A bracket of the URL itself stays.
        let (_, link) = find("https://en.wikipedia.org/wiki/Foo_(bar)", 4).expect("a link");
        assert_eq!(
            link,
            Link::Url("https://en.wikipedia.org/wiki/Foo_(bar)".to_owned())
        );
    }

    #[test]
    fn a_file_with_a_line_is_a_file_link() {
        let line = "error at src/main.rs:12:5";
        let (range, link) = find(line, 12).expect("a link");
        assert_eq!(&line[range], "src/main.rs:12:5");
        assert_eq!(
            link,
            Link::File {
                path: "src/main.rs".to_owned(),
                line: 12,
                column: 5
            }
        );
    }

    #[test]
    fn numbers_and_times_are_not_file_links() {
        assert_eq!(
            find("version 3.4 is out", 9),
            None,
            "a version is not a file"
        );
        assert_eq!(find("at 12:30 we met", 4), None, "a time is not a file");
        assert_eq!(parse("notes:12"), None, "a bare word is not a path");
    }

    #[test]
    fn a_pointer_outside_the_link_finds_nothing() {
        let line = "see https://example.com/ and README.md:3";
        assert_eq!(find(line, 0), None);
        assert_eq!(find(line, 3), None, "a space is not a link");
        let (_, link) = find(line, 32).expect("the file link");
        assert_eq!(link, file("README.md", 3));
    }
}
