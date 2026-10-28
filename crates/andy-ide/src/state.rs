//! Application state that is not the dock layout.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use ide_core::{
    Config, ExternalChange, FileIndex, FileIndexCache, FsEvent, Project, Recent, Tabs, TreeOptions,
    Workspace, WorkspaceState, decide_external_change, watch_project,
};

use crate::editor::EditorBuffer;

/// Something the interface asked for, applied after the frame is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Open (or focus) a file, path relative to the project root.
    OpenFile(PathBuf),
    /// Close the tab at this index, asking first when it has unsaved changes.
    CloseTab(usize),
    /// Close the tab at this index whatever it holds.
    ForceCloseTab(usize),
    /// Save the tab at this index and close it.
    SaveAndCloseTab(usize),
    /// Take the text from disk for a file that changed behind the editor.
    ReloadExternal(PathBuf),
    /// Keep the edits and forget the change on disk.
    KeepMine(PathBuf),
    /// Save every buffer that has changes.
    SaveAll,
    /// Leave without saving anything else.
    Quit,
    /// Go away, do nothing.
    CancelPrompt,
}

/// A question the editor has to ask before it can continue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// The file changed on disk while the buffer had edits of its own.
    ExternalChange { path: PathBuf },
    /// The tab has unsaved edits.
    CloseTab { index: usize },
    /// The window is closing and something is unsaved.
    Quit,
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
    /// The dock layout as it will be written back to `.andy-ide`.
    layout: Option<String>,
    /// A question waiting for an answer; the interface draws it as a modal.
    pub prompt: Option<Prompt>,
    /// Set when the user chose to leave; the next frame closes the window.
    pub quit_requested: bool,
    /// Keeps the watch alive; dropping it stops the events.
    watcher: Option<ide_core::ProjectWatcher>,
    fs_events: Option<Receiver<FsEvent>>,
    next_buffer_id: usize,
    config_dir: PathBuf,
    /// `.andy-ide` of the open project, when it could be created.
    workspace: Option<Workspace>,
    /// Background file walk, so startup is not blocked by it.
    index_job: Option<Receiver<anyhow::Result<(usize, FileIndex)>>>,
    /// What was last written to `workspace.json`.
    last_state: Option<WorkspaceState>,
    last_write: f64,
}

impl AppState {
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
            layout: None,
            prompt: None,
            quit_requested: false,
            watcher: None,
            fs_events: None,
            next_buffer_id: 1,
            config_dir,
            workspace: None,
            index_job: None,
            last_state: None,
            last_write: 0.0,
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

    /// Opens a directory as the project: creates `.andy-ide`, shows the cached
    /// file index immediately and starts rebuilding it in the background.
    pub fn open_project(&mut self, root: &Path) {
        match Project::open(root) {
            Ok(project) => {
                let canonical = project.root().to_path_buf();
                self.project = Some(project);
                self.workspace = Workspace::open(&canonical).ok();
                self.index = None;
                self.index_job = None;
                self.last_state = None;

                if let Some(cache) = self
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.read::<FileIndexCache>("cache/files.json"))
                {
                    let index = cache.index();
                    self.status = format!("{} files from cache", index.len());
                    self.index = Some(index);
                }

                self.start_index_refresh(&canonical);
                self.recent.add(canonical.clone());
                let _ = self.recent.save(&self.config_dir.join("recent.json"));

                self.tree.expanded.clear();
                self.tabs = Tabs::new();
                self.buffers.clear();
                self.start_watching(&canonical.clone());
                self.restore_tabs();
            }
            Err(error) => {
                self.status = format!("cannot open {}: {error}", root.display());
            }
        }
    }

    /// Watches the project tree so files changed behind the editor are noticed.
    fn start_watching(&mut self, root: &Path) {
        let (sender, receiver) = std::sync::mpsc::channel();
        match watch_project(root, sender) {
            Ok(watcher) => {
                self.watcher = Some(watcher);
                self.fs_events = Some(receiver);
            }
            Err(error) => {
                // Without a watcher the editor still works; it just will not
                // notice changes made elsewhere.
                self.watcher = None;
                self.fs_events = None;
                self.status = format!("no file watching: {error}");
            }
        }
    }

    /// Takes every change the watcher has seen since the last frame.
    pub fn poll_fs_events(&mut self) {
        let mut events = Vec::new();
        if let Some(receiver) = self.fs_events.as_ref() {
            loop {
                match receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.fs_events = None;
                        break;
                    }
                }
            }
        }
        for event in events {
            self.handle_fs_event(event);
        }
    }

    /// Decides what to do about one change on disk.
    ///
    /// A file that is not open is ignored: the index and the tree will catch
    /// up on their own, and reopening the project is cheap.
    pub fn handle_fs_event(&mut self, event: FsEvent) {
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return;
        };
        let absolute = event.path().to_path_buf();
        let Ok(rel_path) = absolute.strip_prefix(&root) else {
            return;
        };
        let rel = rel_path.to_path_buf();
        let Some(buffer_id) = self
            .tabs
            .items()
            .iter()
            .find(|item| item.path == rel)
            .map(|item| item.buffer_id)
        else {
            return;
        };

        match event {
            FsEvent::Removed(_) => {
                self.status = format!("{} was removed on disk", display_name(&rel));
            }
            FsEvent::Created(_) | FsEvent::Changed(_) => {
                let Ok(text) = std::fs::read_to_string(&absolute) else {
                    return;
                };
                let Some(buffer) = self.buffers.get_mut(&buffer_id) else {
                    return;
                };
                // The editor writes its own saves atomically, and those come
                // back as events too: same text, nothing to do.
                if buffer.text() == text {
                    return;
                }
                let dirty = buffer.is_dirty();
                match decide_external_change(dirty) {
                    ExternalChange::Reload => {
                        buffer.replace_text(text);
                        self.status = format!("{} reloaded from disk", display_name(&rel));
                        self.sync_dirty();
                    }
                    ExternalChange::Prompt => {
                        self.prompt = Some(Prompt::ExternalChange { path: rel });
                    }
                }
            }
        }
    }

    /// Saves the buffer of one tab.
    fn save_tab(&mut self, index: usize) {
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return;
        };
        let Some(buffer_id) = self.tabs.items().get(index).map(|item| item.buffer_id) else {
            return;
        };
        let Some(buffer) = self.buffers.get_mut(&buffer_id) else {
            return;
        };
        match buffer.save(&root) {
            Ok(()) => self.status = format!("saved {}", display_name(&buffer.path)),
            Err(error) => self.status = format!("{error:#}"),
        }
        self.sync_dirty();
    }

    /// Stops watching the tree; tests hand over their own events instead.
    #[cfg(test)]
    pub fn pause_watching(&mut self) {
        self.watcher = None;
        self.fs_events = None;
    }

    /// Whether any buffer holds unsaved edits.
    pub fn has_unsaved(&self) -> bool {
        self.buffers.values().any(EditorBuffer::is_dirty)
    }

    /// Saves every buffer with changes; returns how many were written.
    pub fn save_all(&mut self) -> usize {
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return 0;
        };
        let mut saved = 0;
        let mut failure = None;
        for buffer in self.buffers.values_mut() {
            if !buffer.is_dirty() {
                continue;
            }
            match buffer.save(&root) {
                Ok(()) => saved += 1,
                Err(error) => failure = Some(format!("{error:#}")),
            }
        }
        self.sync_dirty();
        self.status = failure.unwrap_or_else(|| format!("saved {saved} files"));
        saved
    }

    /// Walks the project in a thread and refreshes both the index and its cache.
    fn start_index_refresh(&mut self, root: &Path) {
        let show_hidden = self.tree.show_hidden;
        let cache_path = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.cache_dir().join("files.json"));
        let root = root.to_path_buf();
        let (sender, receiver) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = FileIndex::build(&root, show_hidden);
            if let Ok(index) = &result
                && let Some(path) = cache_path
                && let Ok(text) = serde_json::to_string(&FileIndexCache::new(index.paths()))
            {
                let _ = ide_core::fs::write_atomic(&path, text.as_bytes());
            }
            let _ = sender.send(result.map(|index| (index.len(), index)));
        });

        self.index_job = Some(receiver);
    }

    /// Takes the freshly built index when the background walk has finished.
    pub fn poll_index(&mut self) {
        let Some(receiver) = self.index_job.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok((count, index))) => {
                self.status = format!("{count} files indexed");
                self.index = Some(index);
                self.index_job = None;
            }
            Ok(Err(error)) => {
                self.status = format!("index failed: {error}");
                self.index_job = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.index_job = None;
            }
        }
    }

    /// Reopens the tabs saved in `.andy-ide/workspace.json`.
    pub fn restore_tabs(&mut self) {
        let Some(state) = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.read_state())
        else {
            return;
        };
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return;
        };
        for path in state.tabs {
            if root.join(&path).is_file() {
                self.open_file(&path);
            }
        }
        if let Some(active) = state.active {
            self.tabs.set_active(active);
        }
        if !self.tabs.is_empty() {
            self.status = format!("{} tabs restored", self.tabs.len());
        }
    }

    /// The dock layout stored with the workspace, if any.
    pub fn stored_layout(&self) -> Option<String> {
        self.workspace
            .as_ref()
            .and_then(|workspace| workspace.read_state().layout)
    }

    /// Writes the workspace file when it changed, at most every 300 ms unless
    /// `force` is set (used when the window closes).
    pub fn save_workspace(&mut self, layout: Option<String>, now: f64, force: bool) {
        if let Some(layout) = layout {
            self.layout = Some(layout);
        }
        let Some(workspace) = self.workspace.as_ref() else {
            return;
        };
        let state = WorkspaceState {
            tabs: self
                .tabs
                .items()
                .iter()
                .map(|item| item.path.clone())
                .collect(),
            active: self.tabs.active(),
            // The layout is serialized once in a while and kept here between
            // saves, so a save that does not carry one cannot wipe it.
            layout: self.layout.clone(),
        };
        if self.last_state.as_ref() == Some(&state) {
            return;
        }
        if !force && now - self.last_write < 0.3 {
            return;
        }
        match workspace.write_state(&state) {
            Ok(()) => {
                self.last_state = Some(state);
                self.last_write = now;
            }
            Err(error) => self.status = format!("cannot save workspace: {error}"),
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

    /// Closes a tab, asking first when it has unsaved changes.
    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if self
            .tabs
            .items()
            .get(index)
            .and_then(|item| self.buffers.get(&item.buffer_id))
            .map(EditorBuffer::is_dirty)
            .unwrap_or(false)
        {
            self.prompt = Some(Prompt::CloseTab { index });
            return;
        }
        self.force_close_tab(index);
    }

    /// Closes a tab whatever it holds.
    pub fn force_close_tab(&mut self, index: usize) {
        let buffer_id = self.tabs.items().get(index).map(|item| item.buffer_id);
        self.prompt = None;
        if self.tabs.close(index) {
            if let Some(buffer_id) = buffer_id {
                self.buffers.remove(&buffer_id);
            }
            self.status = format!("{} tabs", self.tabs.len());
        }
    }

    /// Replaces a buffer with the text on disk, whatever it holds.
    pub fn reload_from_disk(&mut self, rel: &Path) {
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return;
        };
        self.prompt = None;
        let absolute = root.join(rel);
        let Ok(text) = std::fs::read_to_string(&absolute) else {
            self.status = format!("cannot read {}", absolute.display());
            return;
        };
        let Some(buffer_id) = self
            .tabs
            .items()
            .iter()
            .find(|item| item.path == rel)
            .map(|item| item.buffer_id)
        else {
            return;
        };
        if let Some(buffer) = self.buffers.get_mut(&buffer_id) {
            buffer.replace_text(text);
            self.status = format!("{} reloaded from disk", display_name(rel));
        }
        self.sync_dirty();
    }

    /// Applies everything the last frame asked for.
    pub fn apply_pending(&mut self) {
        for action in std::mem::take(&mut self.pending) {
            match action {
                Action::OpenFile(rel) => self.open_file(&rel),
                Action::CloseTab(index) => self.close_tab(index),
                Action::ForceCloseTab(index) => self.force_close_tab(index),
                Action::SaveAndCloseTab(index) => {
                    self.prompt = None;
                    self.save_tab(index);
                    self.force_close_tab(index);
                }
                Action::ReloadExternal(path) => self.reload_from_disk(&path),
                Action::KeepMine(path) => {
                    self.prompt = None;
                    self.status = format!(
                        "kept the editor version of {}; save it or reopen it to see the disk one",
                        display_name(&path)
                    );
                }
                Action::SaveAll => {
                    self.save_all();
                }
                Action::Quit => {
                    self.prompt = None;
                    self.quit_requested = true;
                    self.save_workspace(None, 0.0, true);
                }
                Action::CancelPrompt => self.prompt = None,
            }
        }
    }

    /// Tree options for the current settings.
    pub fn tree_options(&self) -> TreeOptions {
        self.tree.options()
    }
}

/// `$XDG_CONFIG_HOME/andy-ide`, falling back to `~/.config/andy-ide`.
pub(crate) fn config_dir() -> PathBuf {
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
        // The walk happens in a thread; wait for it like the frame loop does.
        for _ in 0..200 {
            if state.index.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            state.poll_index();
        }
        (dir, state)
    }

    #[test]
    fn opening_a_project_builds_the_index_and_clears_tabs() {
        let (dir, state) = state_with_project();
        assert!(state.project.is_some());
        assert_eq!(state.index.as_ref().map(FileIndex::len), Some(1));
        assert_eq!(state.recent.list()[0], dir.path().canonicalize().unwrap());
        assert!(
            dir.path()
                .join(".andy-ide/workspace.json")
                .parent()
                .unwrap()
                .is_dir()
        );
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

#[cfg(test)]
mod workspace_tests {
    use super::*;
    use ide_core::Workspace;

    fn project_with_file() -> (tempfile::TempDir, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "# hi\n").unwrap();
        let config = tempfile::tempdir().unwrap();
        (dir, config)
    }

    fn wait_for_index(state: &mut AppState) {
        for _ in 0..200 {
            if state.index.is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            state.poll_index();
        }
        panic!("index never arrived");
    }

    #[test]
    fn tabs_and_layout_survive_a_restart() {
        let (project, config) = project_with_file();
        let mut state = AppState::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        state.open_file(Path::new("src/main.rs"));
        state.open_file(Path::new("README.md"));
        state.save_workspace(Some("{\"layout\":true}".to_owned()), 10.0, true);

        let mut reopened = AppState::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        assert_eq!(reopened.tabs.len(), 2);
        assert_eq!(reopened.tabs.active(), Some(1));
        assert_eq!(
            reopened.stored_layout().as_deref(),
            Some("{\"layout\":true}")
        );
        assert_eq!(reopened.status, "2 tabs restored");
        wait_for_index(&mut reopened);
    }

    #[test]
    fn vanished_files_are_dropped_on_restore() {
        let (project, config) = project_with_file();
        let mut state = AppState::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        state.open_file(Path::new("src/main.rs"));
        state.save_workspace(None, 10.0, true);

        std::fs::remove_file(project.path().join("src/main.rs")).unwrap();
        let reopened = AppState::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        assert_eq!(reopened.tabs.len(), 0);
    }

    #[test]
    fn the_file_index_is_cached_for_the_next_start() {
        let (project, config) = project_with_file();
        let mut state = AppState::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        wait_for_index(&mut state);
        let cache = project.path().join(".andy-ide/cache/files.json");
        for _ in 0..100 {
            if cache.is_file() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(cache.is_file(), "the walk did not write the cache");

        // A second start shows the cached list before the walk finishes.
        let reopened = AppState::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        let index = reopened.index.as_ref().expect("index from cache");
        assert_eq!(index.len(), 2);
        assert!(reopened.status.contains("from cache"));
    }

    #[test]
    fn workspace_file_is_written_when_state_changes() {
        let (project, config) = project_with_file();
        let mut state = AppState::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        state.open_file(Path::new("README.md"));
        state.save_workspace(None, 5.0, false);

        let workspace = Workspace::open(&project.path().canonicalize().unwrap()).unwrap();
        let stored = workspace.read_state();
        assert_eq!(stored.tabs, vec![PathBuf::from("README.md")]);
        assert_eq!(stored.active, Some(0));
    }
}
