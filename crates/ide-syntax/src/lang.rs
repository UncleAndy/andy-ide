//! Which language a file is written in, and which grammar parses it.

use std::borrow::Cow;
use std::path::Path;

use tree_sitter::Language as TsLanguage;

/// Own TypeScript query: the grammar ships only the TypeScript-specific half.
const TYPESCRIPT_QUERY: &str = include_str!("../queries/typescript.scm");
/// The same query plus the JSX nodes, which only the TSX grammar has.
const TYPESCRIPT_QUERY_WITH_JSX: &str = concat!(
    include_str!("../queries/typescript.scm"),
    include_str!("../queries/tsx_extra.scm")
);

/// A language the editor can highlight.
///
/// TypeScript and JavaScript keep separate grammars because tree-sitter ships
/// them separately; `Tsx` and `JavaScript` share the JSX-capable grammar with
/// their plain variants where the grammars coincide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    Tsx,
    JavaScript,
    Json,
    Toml,
    Markdown,
    Bash,
    Go,
    C,
    Cpp,
}

impl Language {
    /// Short human-readable name, as shown in the status bar.
    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Python => "Python",
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TSX",
            Language::JavaScript => "JavaScript",
            Language::Json => "JSON",
            Language::Toml => "TOML",
            Language::Markdown => "Markdown",
            Language::Bash => "Bash",
            Language::Go => "Go",
            Language::C => "C",
            Language::Cpp => "C++",
        }
    }

    /// Detects the language of a file: extension first, then, for files without
    /// a useful extension, the first line (a shebang).
    pub fn detect(path: &Path, first_line: Option<&str>) -> Option<Language> {
        if let Some(language) = Language::from_path(path) {
            return Some(language);
        }
        first_line.and_then(Language::from_shebang)
    }

    /// Detection by file name and extension only.
    pub fn from_path(path: &Path) -> Option<Language> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        let by_name = match name.as_str() {
            "makefile" | "gnumakefile" | "dockerfile" | ".bashrc" | ".bash_profile"
            | ".profile" | ".zshrc" => Some(Language::Bash),
            _ => None,
        };
        if by_name.is_some() {
            return by_name;
        }

        let extension = path.extension()?.to_string_lossy().to_lowercase();
        let language = match extension.as_str() {
            "rs" => Language::Rust,
            "py" | "pyi" | "pyw" => Language::Python,
            "ts" | "mts" | "cts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "json" => Language::Json,
            "toml" => Language::Toml,
            "md" | "markdown" => Language::Markdown,
            "sh" | "bash" | "zsh" => Language::Bash,
            "go" => Language::Go,
            // A plain `.h` is highlighted as C; C++ headers use `hpp`/`hh`/`hxx`.
            "c" | "h" => Language::C,
            "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" => Language::Cpp,
            _ => return None,
        };
        Some(language)
    }

    /// Detection by interpreter mentioned in a `#!` line.
    pub fn from_shebang(line: &str) -> Option<Language> {
        let line = line.trim_start();
        if !line.starts_with("#!") {
            return None;
        }
        let lower = line.to_lowercase();
        if lower.contains("python") {
            Some(Language::Python)
        } else if lower.contains("node") || lower.contains("deno") {
            Some(Language::JavaScript)
        } else if lower.contains("bash")
            || lower.contains("zsh")
            || lower.ends_with("/sh")
            || lower.contains(" sh ")
        {
            Some(Language::Bash)
        } else {
            None
        }
    }

    /// The tree-sitter grammar for this language.
    pub fn tree_sitter(self) -> TsLanguage {
        let grammar = match self {
            Language::Rust => tree_sitter_rust::LANGUAGE,
            Language::Python => tree_sitter_python::LANGUAGE,
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX,
            Language::JavaScript => tree_sitter_javascript::LANGUAGE,
            Language::Json => tree_sitter_json::LANGUAGE,
            Language::Toml => tree_sitter_toml_ng::LANGUAGE,
            Language::Markdown => tree_sitter_md::LANGUAGE,
            Language::Bash => tree_sitter_bash::LANGUAGE,
            Language::Go => tree_sitter_go::LANGUAGE,
            Language::C => tree_sitter_c::LANGUAGE,
            Language::Cpp => tree_sitter_cpp::LANGUAGE,
        };
        TsLanguage::from(grammar)
    }

    /// The highlight query for this language.
    ///
    /// Most grammars export their query as a constant and those are used as-is;
    /// the constant names differ between crates and are mapped per language
    /// below. TypeScript is the exception: its grammar ships only the
    /// TypeScript-specific patterns (the upstream layout expects the JavaScript
    /// query to be inherited, and that query mentions JSX nodes the plain
    /// TypeScript grammar does not have), so this repository carries its own
    /// `queries/typescript.scm` plus a JSX fragment for TSX.
    /// Returns `Cow` because some languages need two queries concatenated at
    /// run time: the C++ crate ships only the C++-specific supplement and the
    /// upstream layout expects the C query to come first.
    pub fn highlights_query(self) -> Cow<'static, str> {
        match self {
            Language::Rust => Cow::Borrowed(tree_sitter_rust::HIGHLIGHTS_QUERY),
            Language::Python => Cow::Borrowed(tree_sitter_python::HIGHLIGHTS_QUERY),
            Language::TypeScript => Cow::Borrowed(TYPESCRIPT_QUERY),
            Language::Tsx => Cow::Borrowed(TYPESCRIPT_QUERY_WITH_JSX),
            Language::JavaScript => Cow::Borrowed(tree_sitter_javascript::HIGHLIGHT_QUERY),
            Language::Json => Cow::Borrowed(tree_sitter_json::HIGHLIGHTS_QUERY),
            Language::Toml => Cow::Borrowed(tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
            Language::Markdown => Cow::Borrowed(tree_sitter_md::HIGHLIGHT_QUERY_BLOCK),
            Language::Bash => Cow::Borrowed(tree_sitter_bash::HIGHLIGHT_QUERY),
            Language::Go => Cow::Borrowed(tree_sitter_go::HIGHLIGHTS_QUERY),
            Language::C => Cow::Borrowed(tree_sitter_c::HIGHLIGHT_QUERY),
            // The C++ crate ships only the C++-specific supplement; the shared C
            // syntax comes from the C query, as the upstream layout intends.
            Language::Cpp => Cow::Owned(format!(
                "{}\n{}",
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY
            )),
        }
    }
}
