//! Project model, configuration, sessions and filesystem observation for andy-ide.
//!
//! This crate is UI-agnostic: it must never depend on `egui` or `eframe`.

/// Version of the crate, taken from its manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_not_empty() {
        assert!(!super::version().is_empty());
    }
}
