//! Spot checks on realistic samples: every language must distinguish comments,
//! strings and keywords, not merely produce some spans.

use ide_syntax::{Highlighter, Language, TokenKind};

fn kinds(lang: Language, src: &str) -> Vec<TokenKind> {
    let mut h = Highlighter::new(lang);
    h.set_text(src);
    assert!(!h.has_error(), "{lang:?} parses its sample with an error");
    h.spans_for_lines(0, src.lines().count())
        .into_iter()
        .map(|s| s.kind)
        .collect()
}

fn check(lang: Language, src: &str, expected: &[TokenKind]) {
    let found = kinds(lang, src);
    for kind in expected {
        assert!(found.contains(kind), "{lang:?}: нет {kind:?} в {found:?}");
    }
}

#[test]
fn rust_sample() {
    check(
        Language::Rust,
        "// comment\nfn main() {\n    let s = \"x\";\n    let n = 42;\n}\n",
        &[
            TokenKind::Comment,
            TokenKind::Keyword,
            TokenKind::String,
            TokenKind::Number,
        ],
    );
}

#[test]
fn python_sample() {
    check(
        Language::Python,
        "# comment\ndef f():\n    return \"x\"\n",
        &[TokenKind::Comment, TokenKind::Keyword, TokenKind::String],
    );
}

#[test]
fn go_sample() {
    check(
        Language::Go,
        "// comment\npackage main\n\nfunc main() {\n\ts := \"x\"\n}\n",
        &[TokenKind::Comment, TokenKind::Keyword, TokenKind::String],
    );
}

#[test]
fn c_sample() {
    check(
        Language::C,
        "// comment\nint main(void) { char *s = \"x\"; return 0; }\n",
        &[TokenKind::Comment, TokenKind::Type, TokenKind::String],
    );
}

#[test]
fn cpp_sample() {
    check(
        Language::Cpp,
        "// comment\nclass A { public: const char *s = \"x\"; };\n",
        &[TokenKind::Comment, TokenKind::Keyword, TokenKind::String],
    );
}

#[test]
fn bash_sample() {
    check(
        Language::Bash,
        "# comment\nname=\"x\"\necho \"$name\"\n",
        &[TokenKind::Comment, TokenKind::String],
    );
}

#[test]
fn toml_sample() {
    check(
        Language::Toml,
        "# comment\nkey = \"x\"\n[section]\nnumber = 1\n",
        &[TokenKind::Comment, TokenKind::String, TokenKind::Number],
    );
}

#[test]
fn json_sample() {
    check(
        Language::Json,
        "{\"a\": \"x\", \"n\": 1}\n",
        &[TokenKind::String, TokenKind::Number],
    );
}

#[test]
fn markdown_sample() {
    let found = kinds(
        Language::Markdown,
        "# Title\n\nSome **bold** text and `code`.\n\n```rust\nfn main() {}\n```\n",
    );
    assert!(!found.is_empty(), "markdown даёт пустую подсветку");
    assert!(
        found.len() >= 3,
        "markdown размечает слишком мало: {found:?}"
    );
}
