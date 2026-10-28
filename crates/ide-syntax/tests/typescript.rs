//! TypeScript and TSX use a query carried by this repository, because the
//! grammar ships only the TypeScript-specific half of it.

use ide_syntax::{Highlighter, Language, TokenKind};

fn kinds(lang: Language, src: &str) -> Vec<TokenKind> {
    let mut h = Highlighter::new(lang);
    h.set_text(src);
    h.spans_for_lines(0, src.lines().count())
        .into_iter()
        .map(|s| s.kind)
        .collect()
}

#[test]
fn typescript_query_covers_declarations_and_types() {
    let k = kinds(Language::TypeScript, "const x: number = 1;\n");
    assert!(k.contains(&TokenKind::Keyword), "{k:?}");
    assert!(k.contains(&TokenKind::Variable), "{k:?}");
    assert!(k.contains(&TokenKind::Type), "{k:?}");
    assert!(k.contains(&TokenKind::Number), "{k:?}");
}

#[test]
fn tsx_query_covers_jsx_tags_and_attributes() {
    let k = kinds(Language::Tsx, "const el = <div className=\"a\" />;\n");
    assert!(k.contains(&TokenKind::Keyword), "{k:?}");
    assert!(k.contains(&TokenKind::Type), "{k:?}");
    assert!(k.contains(&TokenKind::Attribute), "{k:?}");
    assert!(k.contains(&TokenKind::String), "{k:?}");
}

#[test]
fn typescript_comments_and_methods() {
    let src = "class A {\n  // note\n  greet(): string { return \"hi\"; }\n}\n";
    let k = kinds(Language::TypeScript, src);
    assert!(k.contains(&TokenKind::Comment), "{k:?}");
    assert!(
        k.contains(&TokenKind::Function) || k.contains(&TokenKind::Method),
        "{k:?}"
    );
}
