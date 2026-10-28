//! The project's own directory: `.andy-ide`.
//!
//! Layout and open files live in `.andy-ide/workspace.json`, caches in
//! `.andy-ide/cache/`, which is git-ignored on creation.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Name of the directory the IDE keeps inside a project.
pub const WORKSPACE_DIR: &str = ".andy-ide";
/// File holding the layout and the open tabs.
pub const WORKSPACE_FILE: &str = "workspace.json";
/// Git-ignore written into the directory: the IDE state stays local and is
/// never committed by accident.
pub const GITIGNORE: &str = "*\n";

/// A project's `.andy-ide` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    dir: PathBuf,
}

/// What the IDE remembers between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceState {
    /// Open files, relative to the project root.
    #[serde(default)]
    pub tabs: Vec<PathBuf>,
    /// Index of the active tab in `tabs`.
    #[serde(default)]
    pub active: Option<usize>,
    /// Serialized dock layout; opaque here, because the core must not know egui.
    #[serde(default)]
    pub layout: Option<String>,
}

impl Workspace {
    /// Opens (and creates when missing) the directory for `root`.
    pub fn open(root: &Path) -> Result<Workspace> {
        let dir = root.join(WORKSPACE_DIR);
        std::fs::create_dir_all(dir.join("cache"))
            .with_context(|| format!("cannot create {}", dir.display()))?;

        let gitignore = dir.join(".gitignore");
        if !gitignore.exists() {
            crate::fs::write_atomic(&gitignore, GITIGNORE.as_bytes())
                .with_context(|| format!("cannot write {}", gitignore.display()))?;
        }

        Ok(Workspace { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.dir.join("cache")
    }

    /// Reads a JSON file from the directory; missing or broken files are `None`.
    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let text = std::fs::read_to_string(self.dir.join(name)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Writes a JSON file atomically.
    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let text = serde_json::to_string_pretty(value)?;
        let path = self.dir.join(name);
        crate::fs::write_atomic(&path, text.as_bytes())
            .with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn read_state(&self) -> WorkspaceState {
        self.read(WORKSPACE_FILE).unwrap_or_default()
    }

    pub fn write_state(&self, state: &WorkspaceState) -> Result<()> {
        self.write(WORKSPACE_FILE, state)
    }
}

/// Seconds since the epoch, for cache timestamps.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_creates_the_directory_and_ignores_the_cache() {
        let root = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(root.path()).unwrap();
        assert!(workspace.dir().join("cache").is_dir());
        assert_eq!(
            std::fs::read_to_string(workspace.dir().join(".gitignore")).unwrap(),
            "*\n"
        );
    }

    #[test]
    fn state_round_trips() {
        let root = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(root.path()).unwrap();
        let state = WorkspaceState {
            tabs: vec![PathBuf::from("src/main.rs"), PathBuf::from("README.md")],
            active: Some(1),
            layout: Some("{\"surfaces\":[]}".to_owned()),
        };
        workspace.write_state(&state).unwrap();
        assert_eq!(workspace.read_state(), state);
    }

    #[test]
    fn missing_and_corrupt_files_do_not_fail() {
        let root = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(root.path()).unwrap();
        assert_eq!(workspace.read_state(), WorkspaceState::default());

        std::fs::write(workspace.dir().join(WORKSPACE_FILE), "{not json").unwrap();
        assert_eq!(workspace.read_state(), WorkspaceState::default());
    }

    #[test]
    fn an_existing_gitignore_is_not_overwritten() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(WORKSPACE_DIR)).unwrap();
        std::fs::write(
            root.path().join(WORKSPACE_DIR).join(".gitignore"),
            "custom\n",
        )
        .unwrap();
        let workspace = Workspace::open(root.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.dir().join(".gitignore")).unwrap(),
            "custom\n"
        );
    }
}
