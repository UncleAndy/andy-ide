//! Positions on the wire are lines and UTF-16 code units; the editor works in
//! byte offsets. Both directions live here.
//!
//! UTF-16 is not a detail that can be ignored: a Cyrillic letter is one unit but
//! two bytes, and an emoji is two units and four bytes, so a server that says
//! "column 3" means something else in our text.

use lsp_types::Position;

/// Byte offset of a wire position.
///
/// Positions past the end of a line land at the end of that line, and positions
/// past the end of the text at the end of the text: a server that is a frame
/// behind must not be able to panic the editor.
pub fn position_to_byte(text: &str, position: Position) -> usize {
    let line_start = line_start_byte(text, position.line as usize);
    let line_end = line_end_byte(text, position.line as usize);
    let line = &text[line_start..line_end];

    let mut units = 0u32;
    for (index, character) in line.char_indices() {
        if units >= position.character {
            return line_start + index;
        }
        units += character.len_utf16() as u32;
    }
    line_end
}

/// Wire position of a byte offset.
pub fn byte_to_position(text: &str, byte: usize) -> Position {
    // A caret can sit inside a character only through a bug elsewhere; snapping
    // back keeps the conversion total.
    let mut byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    let prefix = &text[..byte];
    let mut line = 0u32;
    let mut last_break = 0usize;
    for (index, character) in prefix.char_indices() {
        match character {
            '\n' => {
                line += 1;
                last_break = index + 1;
            }
            // A CRLF is one break: the LF that follows counts it.
            '\r' if !prefix[index..].starts_with("\r\n") => {
                line += 1;
                last_break = index + 1;
            }
            _ => {}
        }
    }
    let tail = &prefix[last_break..];
    Position {
        line,
        character: tail.chars().map(|c| c.len_utf16() as u32).sum(),
    }
}

/// Byte where the line starts. A line number past the end gives the end of the text.
fn line_start_byte(text: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    let mut current = 0usize;
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let skip = match bytes[index] {
            b'\n' => 1,
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            b'\r' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        current += 1;
        index += skip;
        if current == line {
            return index;
        }
    }
    text.len()
}

/// Byte just past the line's last character, before its terminator.
fn line_end_byte(text: &str, line: usize) -> usize {
    let start = line_start_byte(text, line);
    if start >= text.len() {
        return text.len();
    }
    let rest = &text[start..];
    let bytes = rest.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => return start + index,
            b'\r' => return start + index,
            _ => index += 1,
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn ascii_positions_are_byte_offsets() {
        let text = "let x = 1;\nlet y = 2;\n";
        assert_eq!(position_to_byte(text, position(0, 0)), 0);
        assert_eq!(position_to_byte(text, position(0, 4)), 4);
        assert_eq!(position_to_byte(text, position(1, 0)), 11);
        assert_eq!(position_to_byte(text, position(1, 4)), 15);
        assert_eq!(byte_to_position(text, 15), position(1, 4));
    }

    #[test]
    fn cyrillic_counts_units_not_bytes() {
        // Each Cyrillic letter is one UTF-16 unit and two bytes.
        let text = "// привет\nlet x = 1;\n";
        let after_comment = position_to_byte(text, position(0, 9));
        assert_eq!(&text[..after_comment], "// привет");
        assert_eq!(byte_to_position(text, after_comment), position(0, 9));

        let second = position_to_byte(text, position(1, 4));
        assert_eq!(&text[second..second + 1], "x");
        assert_eq!(byte_to_position(text, second), position(1, 4));
    }

    #[test]
    fn an_emoji_counts_two_units() {
        let text = "// 🦀 crab\nlet x = 1;\n";
        let after = position_to_byte(text, position(0, 5));
        assert_eq!(&text[..after], "// 🦀");
        assert_eq!(byte_to_position(text, after), position(0, 5));
        // The line below is unaffected by the wider character above.
        assert_eq!(position_to_byte(text, position(1, 0)), 13);
    }

    #[test]
    fn crlf_is_one_line_break() {
        let text = "one\r\ntwo\r\nthree";
        assert_eq!(position_to_byte(text, position(1, 0)), 5);
        assert_eq!(byte_to_position(text, 5), position(1, 0));
        assert_eq!(position_to_byte(text, position(2, 2)), 12);
        assert_eq!(byte_to_position(text, 12), position(2, 2));
    }

    #[test]
    fn a_position_past_the_end_lands_at_the_end() {
        let text = "abc\ndef\n";
        assert_eq!(position_to_byte(text, position(0, 99)), 3, "end of line 0");
        assert_eq!(position_to_byte(text, position(1, 99)), 7, "end of line 1");
        assert_eq!(
            position_to_byte(text, position(99, 0)),
            text.len(),
            "past the text"
        );
    }

    #[test]
    fn a_byte_in_the_middle_of_a_character_snaps_to_its_start() {
        let text = "привет";
        // Byte 1 is inside 'п'; the nearest wire position is column 0.
        assert_eq!(byte_to_position(text, 1), position(0, 0));
        assert_eq!(byte_to_position(text, 2), position(0, 1));
    }
}
