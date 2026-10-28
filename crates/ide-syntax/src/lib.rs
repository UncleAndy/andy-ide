//! tree-sitter language registry and highlighting.
//!
//! This crate is UI-agnostic: it must never depend on `egui` or `eframe`.

pub mod highlight;
pub mod lang;

pub use highlight::{HighlightSpan, Highlighter, TokenKind};
pub use lang::Language;

/// Version of the crate, taken from its manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
