use ide_syntax::{Highlighter, Language, TokenKind};
use std::path::Path;

#[test]
fn detects_by_extension_and_shebang() {
    assert_eq!(
        Language::detect(Path::new("a.rs"), None),
        Some(Language::Rust)
    );
    assert_eq!(
        Language::detect(Path::new("a.tsx"), None),
        Some(Language::Tsx)
    );
    assert_eq!(
        Language::detect(Path::new("run"), Some("#!/usr/bin/env python3")),
        Some(Language::Python)
    );
    assert_eq!(
        Language::detect(Path::new("Makefile"), None),
        Some(Language::Bash)
    );
    assert_eq!(Language::detect(Path::new("a.unknown"), None), None);
}

fn kinds(src: &str, lang: Language) -> Vec<TokenKind> {
    let mut h = Highlighter::new(lang);
    h.set_text(src);
    h.spans_for_lines(0, src.lines().count())
        .into_iter()
        .map(|s| s.kind)
        .collect()
}

#[test]
fn highlights_rust_keywords_and_comments() {
    let k = kinds("fn main() { // hi\n    let x = 1;\n}\n", Language::Rust);
    assert!(k.contains(&TokenKind::Keyword), "{k:?}");
    assert!(k.contains(&TokenKind::Comment), "{k:?}");
    assert!(k.contains(&TokenKind::Number), "{k:?}");
}

#[test]
fn highlights_every_supported_language() {
    let cases = [
        (Language::Rust, "fn f() -> u32 { 1 }\n"),
        (Language::Python, "def f():\n    return 1\n"),
        (Language::TypeScript, "const x: number = 1;\n"),
        (Language::Tsx, "const el = <div className=\"a\" />;\n"),
        (Language::JavaScript, "function f() { return 1; }\n"),
        (Language::Json, "{\"a\": 1}\n"),
        (Language::Toml, "key = \"value\"\n"),
        (Language::Markdown, "# Title\n"),
        (Language::Bash, "echo hi\n"),
        (Language::Go, "package main\n"),
        (Language::C, "int main(void) { return 0; }\n"),
        (Language::Cpp, "class A { public: int x; };\n"),
    ];
    for (lang, src) in cases {
        let spans = kinds(src, lang);
        assert!(!spans.is_empty(), "{lang:?} даёт пустую подсветку");
    }
}

#[test]
fn broken_source_still_highlights_and_reports_error() {
    let src = "fn main( { let x = \"unterminated\n";
    let mut h = Highlighter::new(Language::Rust);
    h.set_text(src);
    assert!(h.has_error());
    assert!(!h.spans_for_lines(0, 1).is_empty());
}

#[test]
fn incremental_edit_matches_full_reparse() {
    let mut h = Highlighter::new(Language::Rust);
    h.set_text("fn a() {}\n");
    let inserted = "fn b() { let y = 2; }\n";
    // Byte 9 is just before the trailing newline; the replacement ends at
    // 9 + len(inserted) in the new text.
    h.edit(9, 9, 9 + inserted.len(), inserted);

    let incremental: Vec<_> = h
        .spans_for_lines(0, 3)
        .into_iter()
        .map(|s| (s.start_byte, s.kind))
        .collect();

    let mut full = Highlighter::new(Language::Rust);
    full.set_text(h.text());
    let whole: Vec<_> = full
        .spans_for_lines(0, 3)
        .into_iter()
        .map(|s| (s.start_byte, s.kind))
        .collect();

    assert_eq!(incremental, whole);
}

#[test]
fn only_requested_lines_are_returned() {
    let mut h = Highlighter::new(Language::Python);
    h.set_text("def a():\n    return 1\n\n\ndef b():\n    return 2\n");
    let first = h.spans_for_lines(0, 1);
    let last = h.spans_for_lines(4, 5);
    assert!(!first.is_empty());
    assert!(!last.is_empty());
    let first_end = h.line_byte_range(0, 1).end;
    assert!(first.iter().all(|s| s.start_byte < first_end));
    assert!(
        last.iter()
            .all(|s| s.end_byte > h.line_byte_range(4, 5).start)
    );
}
