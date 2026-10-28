//! Repository state, diffs and history for andy-ide.
//!
//! This crate is UI-agnostic: it must never depend on `egui` or `eframe`.

/// Version of the crate, taken from its manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
