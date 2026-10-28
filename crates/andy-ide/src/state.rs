//! Application state that is not the dock layout.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use ide_core::{Config, FileIndex, Project, Recent, Session, Tabs, TreeOptions};

use crate::editor::EditorBuffer;

/// Something the interface asked for, applied after the frame is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Open (or focus) a file, path relative to the project root.
    OpenFile(PathBuf),
    /// Close the tab at this index, asking first when it has unsaved changes.
    CloseTab(usize),
}

/// Which directories the project tree shows open, and how it is shown.
#[derive(Debug, Default)]
pub struct TreeState {
    pub expanded: BTreeSet<PathBuf>,
    pub show_hidden: bool,
}

impl TreeState {
    fn options(&self) -> TreeOptions {
        TreeOptions {
            show_hidden: self.show_hidden,
        }
    }
}

/// Everything except the dock layout.
pub struct AppState {
    pub config: Config,
    pub recent: Recent,
    pub project: Option<Project>,
    pub tree: TreeState,
    pub tabs: Tabs,
    /// Open documents, keyed by the tab's buffer id.
    pub buffers: HashMap<usize, EditorBuffer>,
    pub index: Option<FileIndex>,
    /// Text shown in the status bar.
    pub status: String,
    /// Requests collected while drawing, applied afterwards.
    pub pending: Vec<Action>,
    next_buffer_id: usize,
    config_dir: PathBuf,
}

impl AppState {
    /// Loads configuration and the recent list, then opens what was asked for.
    pub fn new(project: Option<PathBuf>, file: Option<PathBuf>) -> Self {
        Self::with_config_dir(config_dir(), project, file)
    }

    /// Same as [`AppState::new`] but with an explicit configuration directory,
    /// which keeps tests away from the real `~/.config`.
    pub fn with_config_dir(
        config_dir: PathBuf,
        project: Option<PathBuf>,
        file: Option<PathBuf>,
    ) -> Self {
        let config = Config::load_or_default(&config_dir.join("config.toml"));
        let recent = Recent::load(&config_dir.join("recent.json"));

        let mut state = Self {
            tree: TreeState {
                show_hidden: config.show_hidden,
                ..TreeState::default()
            },
            config,
            recent,
            project: None,
            tabs: Tabs::new(),
            buffers: HashMap::new(),
            index: None,
            status: String::new(),
            pending: Vec::new(),
            next_buffer_id: 1,
            config_dir,
        };

        if let Some(root) = project {
            state.open_project(&root);
        }
        if let Some(path) = file
            && let Some(project) = state.project.as_ref()
        {
            let rel = path
                .strip_prefix(project.root())
                .map(Path::to_path_buf)
                .unwrap_or(path);
            state.open_file(&rel);
        }
        state
    }

    /// Opens a directory as the project, rebuilding the file index.
    pub fn open_project(&mut self, root: &Path) {
        match Project::open(root) {
            Ok(project) => {
                match FileIndex::build(project.root(), self.tree.show_hidden) {
                    Ok(index) => {
                        self.status = format!("{} files", index.len());
                        self.index = Some(index);
                    }
                    Err(error) => {
                        self.index = None;
                        self.status = format!("index failed: {error}");
                    }
                }
                let canonical = project.root().to_path_buf();
                self.recent.add(canonical.clone());
                let _ = self.recent.save(&self.config_dir.join("recent.json"));
                self.project = Some(project);
                self.tree.expanded.clear();
                self.tabs = Tabs::new();
                self.buffers.clear();
            }
            Err(error) => {
                self.status = format!("cannot open {}: {error}", root.display());
            }
        }
    }

    /// Opens a file in a tab and activates it, or focuses its existing tab.
    pub fn open_file(&mut self, rel: &Path) {
        if let Some(index) = self.tabs.find(rel) {
            self.tabs.set_active(index);
            self.status = display_name(rel);
            return;
        }
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            self.status = "no project open".to_owned();
            return;
        };
        match EditorBuffer::load(&root, rel) {
            Ok(buffer) => {
                let buffer_id = self.next_buffer_id;
                self.next_buffer_id += 1;
                self.buffers.insert(buffer_id, buffer);
                let index = self.tabs.open(rel.to_path_buf(), buffer_id);
                self.tabs.set_active(index);
                self.status = display_name(rel);
            }
            Err(error) => self.status = format!("{error:#}"),
        }
    }

    /// The buffer of the active tab.
    pub fn active_buffer(&self) -> Option<&EditorBuffer> {
        let index = self.tabs.active()?;
        let buffer_id = self.tabs.items().get(index)?.buffer_id;
        self.buffers.get(&buffer_id)
    }

    /// The buffer of the active tab, for editing.
    pub fn active_buffer_mut(&mut self) -> Option<&mut EditorBuffer> {
        let index = self.tabs.active()?;
        let buffer_id = self.tabs.items().get(index)?.buffer_id;
        self.buffers.get_mut(&buffer_id)
    }

    /// Line and column of the active caret, for the status bar.
    pub fn caret_position(&self) -> Option<(usize, usize)> {
        self.active_buffer().map(|buffer| {
            let (line, col) = buffer.line_col();
            (line + 1, col + 1)
        })
    }

    /// Name of the active buffer's language, if it has one.
    pub fn active_language(&self) -> Option<&'static str> {
        self.active_buffer()
            .and_then(|buffer| buffer.highlighter.as_ref())
            .map(|highlighter| highlighter.language().name())
    }

    /// Saves the active buffer and returns the outcome for the status bar.
    pub fn save_active(&mut self) {
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return;
        };
        let Some(index) = self.tabs.active() else {
            return;
        };
        let Some(buffer_id) = self.tabs.items().get(index).map(|item| item.buffer_id) else {
            return;
        };
        let Some(buffer) = self.buffers.get_mut(&buffer_id) else {
            return;
        };
        match buffer.save(&root) {
            Ok(()) => {
                self.status = format!("saved {}", display_name(&buffer.path));
                self.sync_dirty();
            }
            Err(error) => self.status = format!("{error:#}"),
        }
    }

    /// Copies each buffer's dirty flag into its tab, so the strip matches.
    pub fn sync_dirty(&mut self) {
        let flags: Vec<(usize, bool)> = self
            .tabs
            .items()
            .iter()
            .map(|item| {
                let dirty = self
                    .buffers
                    .get(&item.buffer_id)
                    .map(EditorBuffer::is_dirty)
                    .unwrap_or(false);
                (item.buffer_id, dirty)
            })
            .collect();
        for (buffer_id, dirty) in flags {
            self.tabs.dirty_flag(buffer_id, dirty);
        }
    }

    /// Closes a tab, refusing when it has unsaved changes.
    pub fn close_tab(&mut self, index: usize) {
        let buffer_id = self.tabs.items().get(index).map(|item| item.buffer_id);
        if self.tabs.close(index) {
            if let Some(buffer_id) = buffer_id {
                self.buffers.remove(&buffer_id);
            }
            self.status = format!("{} tabs", self.tabs.len());
        } else {
            self.status = format!(
                "{} has unsaved changes",
                self.tabs
                    .items()
                    .get(index)
                    .map(|item| display_name(&item.path))
                    .unwrap_or_default()
            );
        }
    }

    /// Applies everything the last frame asked for.
    pub fn apply_pending(&mut self) {
        for action in std::mem::take(&mut self.pending) {
            match action {
                Action::OpenFile(rel) => self.open_file(&rel),
                Action::CloseTab(index) => self.close_tab(index),
            }
        }
    }

    /// Fills the tab list from a saved session, dropping paths that are gone.
    pub fn restore_session(&mut self) {
        let session = Session::load(&self.config_dir.join("session.json"));
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return;
        };
        for tab in session.tabs {
            let absolute = root.join(&tab.path);
            if absolute.is_file() {
                self.open_file(&tab.path);
            }
        }
    }

    /// Tree options for the current settings.
    pub fn tree_options(&self) -> TreeOptions {
        self.tree.options()
    }
}

/// `$XDG_CONFIG_HOME/andy-ide`, falling back to `~/.config/andy-ide`.
fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("andy-ide")
}

/// The file name of a path, for tab titles and messages.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn state_with_project() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let mut state = AppState::with_config_dir(config_dir.path().to_path_buf(), None, None);
        state.open_project(dir.path());
        (dir, state)
    }

    #[test]
    fn opening_a_project_builds_the_index_and_clears_tabs() {
        let (dir, state) = state_with_project();
        assert!(state.project.is_some());
        assert_eq!(state.index.as_ref().map(FileIndex::len), Some(1));
        assert_eq!(state.recent.list()[0], dir.path().canonicalize().unwrap());
    }

    #[test]
    fn opening_the_same_file_twice_keeps_one_tab() {
        let (_dir, mut state) = state_with_project();
        state.open_file(Path::new("src/main.rs"));
        state.open_file(Path::new("src/main.rs"));
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(state.status, "main.rs");
    }

    #[test]
    fn missing_directory_reports_instead_of_panicking() {
        let config_dir = tempfile::tempdir().unwrap();
        let mut state = AppState::with_config_dir(config_dir.path().to_path_buf(), None, None);
        state.open_project(Path::new("/nonexistent/andy-ide-test"));
        assert!(state.project.is_none());
        assert!(state.status.contains("cannot open"));
    }
}
