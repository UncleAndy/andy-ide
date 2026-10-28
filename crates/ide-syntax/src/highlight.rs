//! Syntax highlighting on top of tree-sitter.
//!
//! The highlighter owns the source text and the parse tree. Parsing is
//! incremental: [`Highlighter::edit`] describes one replacement in bytes and the
//! previous tree is reused, so the cost of a keystroke does not grow with file
//! size.

use std::ops::Range;

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
    query: Option<Query>,
    tree: Option<tree_sitter::Tree>,
    source: String,
    line_starts: Vec<usize>,
    spans: Vec<HighlightSpan>,
}

impl std::fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Highlighter")
            .field("lang", &self.lang)
            .field("bytes", &self.source.len())
            .field("spans", &self.spans.len())
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
        let query_source = lang.highlights_query();
        let query = match Query::new(&ts, &query_source) {
            Ok(query) => Some(query),
            Err(error) => {
                tracing::error!(?lang, %error, "highlight query does not compile");
                None
            }
        };

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
            spans: Vec::new(),
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
        let previous = self.tree.take();
        drop(previous);
        self.tree = self
            .parser
            .as_mut()
            .and_then(|parser| parser.parse(&self.source, None));
        self.recompute_spans();
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
        let previous = self.tree.take();
        self.tree = self
            .parser
            .as_mut()
            .and_then(|parser| parser.parse(&self.source, previous.as_ref()));
        self.recompute_spans();
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
        let range = self.line_byte_range(first_line, last_line);
        if range.is_empty() {
            return Vec::new();
        }

        let mut window: Vec<&HighlightSpan> = Vec::new();
        for span in &self.spans {
            if span.end_byte <= range.start {
                continue;
            }
            if span.start_byte >= range.end {
                // Spans are sorted by start, so the rest are past the window.
                break;
            }
            window.push(span);
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

    /// Rebuilds the span list for the whole tree.
    ///
    /// Runs once per change, not per painted frame: the render loop only filters
    /// this list by the visible line range.
    fn recompute_spans(&mut self) {
        self.spans.clear();
        let (Some(tree), Some(query)) = (self.tree.as_ref(), self.query.as_ref()) else {
            return;
        };

        let mut cursor = QueryCursor::new();
        let mut spans = Vec::new();
        let mut matches = cursor.matches(query, tree.root_node(), self.source.as_bytes());
        // tree-sitter exposes matches as a streaming iterator, not an `Iterator`.
        while let Some(found) = matches.next() {
            for capture in found.captures() {
                let Some(kind) = kind_for_capture(query.capture_names()[capture.index as usize])
                else {
                    continue;
                };
                let node = capture.node;
                spans.push(HighlightSpan {
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    kind: refine_kind(node.kind(), kind),
                    priority: found.pattern_index as u32,
                });
            }
        }
        spans.sort_by(|a, b| {
            a.start_byte
                .cmp(&b.start_byte)
                .then_with(|| b.end_byte.cmp(&a.end_byte))
        });
        self.spans = spans;
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
