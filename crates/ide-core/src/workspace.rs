//! The project's own directory: `.andy-ide`.
//!
//! Layout and open files live in `.andy-ide/workspace.json`, caches in
//! `.andy-ide/cache/`, which is git-ignored on creation.

use std::collections::BTreeMap;
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
    /// The panels that are put away, by the name the window gives them. Names rather than the
    /// window's own panel type, for the same reason: the core must not know egui, and a saved
    /// workspace should outlive a build that renames one of them.
    ///
    /// `None` is a workspace that has never said, which is not the same as one that has said
    /// nothing is put away: the first is a project opened for the first time and wants this
    /// build's own answer, the second is a person who has shown every panel and means it.
    #[serde(default)]
    pub hidden: Option<Vec<String>>,
    /// The agent that was on duty when the window closed, by the name the window gives it.
    /// The connection to it is opened again on the next launch, the way the conversation it
    /// kept is picked up. A workspace that never connected to one has no line for it at all
    /// rather than an empty one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Where the caret stood in each open file, by line and column — both counted from zero, in
    /// characters. A line and a column outlive an edit elsewhere in a file better than a
    /// character index does: a character index points into the middle of another word once
    /// something above it grows or shrinks. The map is left out of the file when it is empty,
    /// so a project that has never had a caret open reads as it always did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub carets: BTreeMap<PathBuf, (usize, usize)>,
    /// The View switches of the window, by the name the window gives them — the same reason the
    /// panels are stored by name: the core must not know egui, and a saved workspace should
    /// outlive a build that renames one. A switch this build does not know is left in the file
    /// rather than dropped for a build that never comes, and a project that has never said has
    /// no line here at all.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub view: BTreeMap<String, bool>,
    /// What was known about the context of the agent's conversation when the workspace was last
    /// written: the last figure the agent sent.
    ///
    /// It is kept with the project so the panel has something to show the moment a session comes
    /// back. An agent names the size of its context window only after the first exchange of a
    /// restored session, so without this the panel was blank until something was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextSpan>,
}

/// Tokens spent, and the window they were spent from.
///
/// The window is the agent's own figure and may be missing: an agent that never names one gets a
/// plain token count instead of a share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextSpan {
    /// Tokens spent so far.
    pub used: u64,
    /// The window they are spent from, if the agent named one.
    pub window: Option<u64>,
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
            hidden: Some(vec!["diff".to_owned()]),
            agent: Some("stand-in".to_owned()),
            carets: BTreeMap::from([(PathBuf::from("src/main.rs"), (12, 4))]),
            view: BTreeMap::from([("hints".to_owned(), false)]),
            context: None,
        };
        workspace.write_state(&state).unwrap();
        assert_eq!(workspace.read_state(), state);
    }

    #[test]
    fn a_workspace_that_never_had_a_caret_has_no_line_for_one() {
        // The map is left out rather than written empty: an older file and a project that has
        // only ever been opened read the same way.
        let root = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(root.path()).unwrap();
        workspace
            .write_state(&WorkspaceState {
                tabs: vec![PathBuf::from("src/main.rs")],
                active: Some(0),
                layout: None,
                hidden: None,
                agent: None,
                carets: BTreeMap::new(),
                view: BTreeMap::new(),
                context: None,
            })
            .unwrap();
        let written = std::fs::read_to_string(workspace.dir().join(WORKSPACE_FILE)).unwrap();
        assert!(
            !written.contains("carets"),
            "an empty map of carets was written: {written}"
        );
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
