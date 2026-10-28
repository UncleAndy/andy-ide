//! Syntax highlighting on top of tree-sitter.
//!
//! The highlighter owns the source text and the parse tree. Parsing is
//! incremental: [`Highlighter::edit`] describes one replacement in bytes and the
//! previous tree is reused, so the cost of a keystroke does not grow with file
//! size.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Point, Query, QueryCursor};

use crate::lang::Language;

/// What a highlighted range is, from the point of view of a colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Keyword,
    Type,
    Function,
    Method,
    Variable,
    Constant,
    String,
    Escape,
    Number,
    Comment,
    DocComment,
    Attribute,
    Operator,
    Punctuation,
    Error,
}

/// A byte range with a token kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub kind: TokenKind,
    /// Position of the query pattern that produced the capture. Tree-sitter
    /// conventions put the specific patterns after the general ones, so a
    /// later pattern wins where two captures land on the same bytes.
    pub priority: u32,
}

/// Incremental highlighter for one document.
pub struct Highlighter {
    lang: Language,
    parser: Option<tree_sitter::Parser>,
    query: Option<Arc<Query>>,
    tree: Option<tree_sitter::Tree>,
    source: String,
    line_starts: Vec<usize>,
    /// Bumped on every change, so a cached window knows it is stale.
    generation: u64,
    /// The last window that was queried. Painting asks for the same lines
    /// frame after frame, and re-running the query would cost more than the
    /// whole rest of the frame.
    window: RefCell<WindowCache>,
}

struct WindowCache {
    generation: u64,
    range: Range<usize>,
    spans: Vec<HighlightSpan>,
}

impl Default for WindowCache {
    fn default() -> Self {
        Self {
            generation: u64::MAX,
            range: 0..0,
            spans: Vec::new(),
        }
    }
}

impl std::fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Highlighter")
            .field("lang", &self.lang)
            .field("bytes", &self.source.len())
            .field("spans", &self.window.borrow().spans.len())
            .finish()
    }
}

impl Highlighter {
    /// Builds a highlighter for `lang`.
    ///
    /// A grammar whose query does not compile against its own grammar leaves the
    /// highlighter without spans and logs an error, rather than panicking inside
    /// a render loop.
    pub fn new(lang: Language) -> Self {
        let ts = lang.tree_sitter();
        let query = compile_query(lang, &ts);

        let mut parser = tree_sitter::Parser::new();
        let parser = match parser.set_language(&ts) {
            Ok(()) => Some(parser),
            Err(error) => {
                tracing::error!(?lang, %error, "grammar rejected by the parser");
                None
            }
        };

        Self {
            lang,
            parser,
            query,
            tree: None,
            source: String::new(),
            line_starts: vec![0],
            generation: 0,
            window: RefCell::new(WindowCache::default()),
        }
    }

    pub fn language(&self) -> Language {
        self.lang
    }

    /// The text being highlighted.
    pub fn text(&self) -> &str {
        &self.source
    }

    /// Replaces the whole text and reparses from scratch.
    pub fn set_text(&mut self, text: &str) {
        self.source.clear();
        self.source.push_str(text);
        self.compute_line_starts();
        self.generation = self.generation.wrapping_add(1);
        let previous = self.tree.take();
        drop(previous);
        self.tree = self
            .parser
            .as_mut()
            .and_then(|parser| parser.parse(&self.source, None));
    }

    /// Applies one replacement and reparses incrementally.
    ///
    /// `byte_start..old_end_byte` in the old text is replaced by `text`, which
    /// ends at `new_end_byte` in the new text.
    pub fn edit(
        &mut self,
        byte_start: usize,
        old_end_byte: usize,
        new_end_byte: usize,
        text: &str,
    ) {
        if byte_start > self.source.len() || old_end_byte > self.source.len() {
            self.set_text(&self.source.clone());
            return;
        }
        let start_position = point_at(&self.source, byte_start);
        let old_end_position = point_at(&self.source, old_end_byte);

        let mut updated = String::with_capacity(
            self.source.len() + text.len() - (old_end_byte - byte_start).min(self.source.len()),
        );
        updated.push_str(&self.source[..byte_start]);
        updated.push_str(text);
        updated.push_str(&self.source[old_end_byte..]);
        let new_end_position = point_at(&updated, new_end_byte);

        if let Some(tree) = self.tree.as_mut() {
            tree.edit(&InputEdit {
                start_byte: byte_start,
                old_end_byte,
                new_end_byte,
                start_position,
                old_end_position,
                new_end_position,
            });
        }

        self.source = updated;
        self.compute_line_starts();
        self.generation = self.generation.wrapping_add(1);
        let previous = self.tree.take();
        self.tree = self
            .parser
            .as_mut()
            .and_then(|parser| parser.parse(&self.source, previous.as_ref()));
    }

    /// The text the tree was parsed from; equal to the buffer text when in sync.
    pub fn source_text(&self) -> &str {
        &self.source
    }

    /// Highlight ranges intersecting lines `first_line..=last_line`.
    ///
    /// Spans covering the given lines, as a partition: sorted, non-overlapping,
    /// and covering exactly the bytes that have a colour.
    ///
    /// Captures nest — a macro name sits inside an identifier, a doc comment
    /// inside a comment — and painting them in order would colour some bytes
    /// twice. The window is painted once per byte instead, so the innermost
    /// capture wins.
    pub fn spans_for_lines(&self, first_line: usize, last_line: usize) -> Vec<HighlightSpan> {
        let range = self.window_range(first_line, last_line);
        if range.is_empty() {
            return Vec::new();
        }
        {
            let cache = self.window.borrow();
            if cache.generation == self.generation && cache.range == range {
                return cache.spans.clone();
            }
        }

        let spans = self.query_window(range.clone());
        *self.window.borrow_mut() = WindowCache {
            generation: self.generation,
            range,
            spans: spans.clone(),
        };
        spans
    }

    /// Runs the query over one byte range and turns the captures into a
    /// partition of that range.
    ///
    /// Only the visible window is queried: scanning the whole tree costs more
    /// than painting the frame, and the answer is needed for the window alone.
    fn query_window(&self, range: Range<usize>) -> Vec<HighlightSpan> {
        let (Some(tree), Some(query)) = (self.tree.as_ref(), self.query.as_ref()) else {
            return Vec::new();
        };

        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut window = Vec::new();
        let mut matches = cursor.matches(query, tree.root_node(), self.source.as_bytes());
        while let Some(found) = matches.next() {
            for capture in found.captures() {
                let Some(kind) = kind_for_capture(query.capture_names()[capture.index as usize])
                else {
                    continue;
                };
                let node = capture.node;
                if node.end_byte() <= range.start || node.start_byte() >= range.end {
                    continue;
                }
                window.push(HighlightSpan {
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    kind: refine_kind(node.kind(), kind),
                    priority: found.pattern_index as u32,
                });
            }
        }
        // Paint in query order: a later pattern covers the bytes of an earlier
        // one, which is how tree-sitter highlighting is meant to resolve this.
        window.sort_by_key(|span| span.priority);

        let mut kinds: Vec<Option<TokenKind>> = vec![None; range.len()];
        for span in window {
            let start = span.start_byte.max(range.start) - range.start;
            let end = span.end_byte.min(range.end) - range.start;
            for slot in &mut kinds[start..end] {
                *slot = Some(span.kind);
            }
        }
        snap_to_characters(&self.source, range.clone(), &mut kinds);

        let mut spans = Vec::new();
        let mut index = 0;
        while index < kinds.len() {
            let Some(kind) = kinds[index] else {
                index += 1;
                continue;
            };
            let start = index;
            while index < kinds.len() && kinds[index] == Some(kind) {
                index += 1;
            }
            spans.push(HighlightSpan {
                start_byte: range.start + start,
                end_byte: range.start + index,
                kind,
                priority: 0,
            });
        }
        spans
    }

    /// True when the parse tree contains an error node.
    pub fn has_error(&self) -> bool {
        self.tree
            .as_ref()
            .map(|tree| tree.root_node().has_error())
            .unwrap_or(false)
    }

    /// Byte range covered by `first_line..=last_line`, clamped to the text.
    /// The byte window for the given lines, inside the parsed text and on
    /// character boundaries: the lines of a small stale table must not turn
    /// into a slice outside the text.
    fn window_range(&self, first_line: usize, last_line: usize) -> Range<usize> {
        let range = self.line_byte_range(first_line, last_line);
        let len = self.source.len();
        let mut start = range.start.min(len);
        while start > 0 && !self.source.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = range.end.clamp(start, len);
        while end < len && !self.source.is_char_boundary(end) {
            end += 1;
        }
        start..end
    }

    pub fn line_byte_range(&self, first_line: usize, last_line: usize) -> Range<usize> {
        let start = self
            .line_starts
            .get(first_line)
            .copied()
            .unwrap_or(self.source.len());
        let end = self
            .line_starts
            .get(last_line + 1)
            .copied()
            .unwrap_or(self.source.len());
        start..end.max(start)
    }

    fn compute_line_starts(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        for (idx, byte) in self.source.bytes().enumerate() {
            // A lone `\r` breaks a line too; `\r\n` is one break, not two.
            let lone_return = byte == b'\r' && self.source.as_bytes().get(idx + 1) != Some(&b'\n');
            if byte == b'\n' || lone_return {
                self.line_starts.push(idx + 1);
            }
        }
    }
}

/// Maps a tree-sitter capture name such as `@function.method` to a token kind.
fn kind_for_capture(name: &str) -> Option<TokenKind> {
    let name = name.trim_start_matches('@');
    let base = name.split('.').next().unwrap_or(name);
    let kind = match base {
        "keyword" | "conditional" | "repeat" => TokenKind::Keyword,
        "type" | "constructor" | "tag" | "namespace" => TokenKind::Type,
        "function" => {
            if name.contains("method") {
                TokenKind::Method
            } else {
                TokenKind::Function
            }
        }
        "method" => TokenKind::Method,
        "variable" | "property" | "field" => TokenKind::Variable,
        "constant" | "boolean" => TokenKind::Constant,
        "string" | "text" | "char" | "character" => TokenKind::String,
        "escape" => TokenKind::Escape,
        "number" | "float" | "integer" => TokenKind::Number,
        "comment" => {
            if name.contains("documentation") || name.contains("doc") {
                TokenKind::DocComment
            } else {
                TokenKind::Comment
            }
        }
        "attribute" | "annotation" | "decorator" | "markup" => TokenKind::Attribute,
        "operator" => TokenKind::Operator,
        "punctuation" | "delimiter" | "bracket" => TokenKind::Punctuation,
        "label" => TokenKind::Function,
        "error" => TokenKind::Error,
        _ => return None,
    };
    Some(kind)
}

/// Some grammars capture numeric literals as `@constant.builtin` (Rust does).
/// The node name is the reliable signal, so the kind is corrected here.
fn refine_kind(node_kind: &str, kind: TokenKind) -> TokenKind {
    let numeric = ["integer", "float", "number", "numeric"]
        .iter()
        .any(|needle| node_kind.contains(needle));
    if numeric { TokenKind::Number } else { kind }
}

/// Byte offset to a `Point`, counting columns in bytes as tree-sitter does.
fn point_at(source: &str, byte: usize) -> Point {
    let byte = byte.min(source.len());
    let prefix = &source[..byte];
    let row = prefix.bytes().filter(|b| *b == b'\n').count();
    let column = match prefix.rfind('\n') {
        Some(newline) => prefix.len() - newline - 1,
        None => prefix.len(),
    };
    Point { row, column }
}

/// Compiles a highlight query once per language and shares it.
///
/// Compiling is expensive — tens of milliseconds — while opening a second file
/// of the same language needs the very same query. The result, including
/// failure, is kept: a grammar that rejects its own query would otherwise log
/// the same error for every file.
fn compile_query(lang: Language, ts: &tree_sitter::Language) -> Option<Arc<Query>> {
    static QUERIES: OnceLock<Mutex<HashMap<Language, Option<Arc<Query>>>>> = OnceLock::new();
    let cache = QUERIES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().ok()?.get(&lang) {
        return cached.clone();
    }

    let source = lang.highlights_query();
    let compiled = match Query::new(ts, &source) {
        Ok(query) => Some(Arc::new(query)),
        Err(error) => {
            tracing::error!(?lang, %error, "highlight query does not compile");
            None
        }
    };
    cache
        .lock()
        .expect("query cache poisoned")
        .insert(lang, compiled.clone());
    compiled
}

/// Colours whole characters: a capture that starts or ends inside a multi-byte
/// character takes the rest of that character with it.
///
/// tree-sitter can hand out such ranges after an incremental reparse — the byte
/// grid would then hold half a letter, and drawing half a letter panics in the
/// layout. One character of a slightly wrong colour is invisible; a panic is
/// not.
fn snap_to_characters(source: &str, range: Range<usize>, kinds: &mut [Option<TokenKind>]) {
    let text = &source[range.start..range.end];
    let mut fixed = 0usize;
    for (offset, character) in text.char_indices() {
        let width = character.len_utf8();
        if width == 1 {
            continue;
        }
        let slots = &kinds[offset..offset + width];
        let Some(kind) = slots.iter().find_map(|slot| *slot) else {
            continue;
        };
        if slots.iter().all(|slot| *slot == Some(kind)) {
            continue;
        }
        fixed += 1;
        kinds[offset..offset + width].fill(Some(kind));
    }
    if fixed > 0 {
        tracing::debug!(
            fixed,
            start = range.start,
            "colour extended over whole characters"
        );
    }
}

#[cfg(test)]
mod character_tests {
    use super::*;

    fn grid(text: &str) -> Vec<Option<TokenKind>> {
        vec![None; text.len()]
    }

    #[test]
    fn half_a_capture_takes_the_whole_character() {
        let text = "// ПП x";
        let mut kinds = grid(text);
        // Colour only the second byte of the first letter, as a stale tree could.
        kinds[4] = Some(TokenKind::String);
        snap_to_characters(text, 0..text.len(), &mut kinds);
        let coloured: Vec<usize> = kinds
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.is_some())
            .map(|(index, _)| index)
            .collect();
        assert_eq!(coloured, vec![3, 4], "the letter at bytes 3..5 stays whole");
    }

    #[test]
    fn a_capture_that_cuts_a_character_does_not_leave_half_of_it() {
        let text = "let s = \"Привет\";";
        let open = text.find('П').unwrap();
        let mut kinds = grid(text);
        for slot in &mut kinds[open + 1..open + 7] {
            *slot = Some(TokenKind::String);
        }
        snap_to_characters(text, 0..text.len(), &mut kinds);

        let mut index = 0;
        while index < kinds.len() {
            if kinds[index].is_none() {
                index += 1;
                continue;
            }
            let start = index;
            while index < kinds.len() && kinds[index].is_some() {
                index += 1;
            }
            assert!(
                text.is_char_boundary(start) && text.is_char_boundary(index),
                "coloured run {start}..{index} splits a character"
            );
        }
    }
}
