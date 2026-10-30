//! Rope-backed text buffer, cursors and undo history.
//!
//! This crate is UI-agnostic: it must never depend on `egui` or `eframe`.

pub mod cursor;
pub mod document;
pub mod history;
pub mod search;

pub use cursor::Cursor;
pub use document::Document;
pub use history::{Edit, EditKind, HISTORY_LIMIT, History};
pub use search::{Hit, Needle};

/// Version of the crate, taken from its manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
