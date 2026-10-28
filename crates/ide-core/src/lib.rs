//! Project model, configuration, sessions and filesystem observation for andy-ide.
//!
//! This crate is UI-agnostic: it must never depend on `egui` or `eframe`.

pub mod config;
pub mod fs;
pub mod project;
pub mod quickopen;
pub mod session;
pub mod tabs;
pub mod watch;
pub mod workspace;

pub use config::{Config, Theme};
pub use project::{Entry, EntryKind, Project, TreeOptions};
pub use quickopen::{FileIndex, FileIndexCache, SearchHit};
pub use session::{RECENT_LIMIT, Recent, Session, TabState};
pub use tabs::{TabItem, Tabs};
pub use watch::{
    ExternalChange, FsEvent, ProjectWatcher, decide_external_change, watch_extra_file,
    watch_project,
};
pub use workspace::{WORKSPACE_DIR, Workspace, WorkspaceState};

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
