//! Application state that is not the dock layout.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use ide_agents::Usage;
use ide_core::{
    Config, ContextSpan, ExternalChange, FileIndex, FileIndexCache, FsEvent, Project, Recent, Tabs,
    TreeOptions, Workspace, WorkspaceState, decide_external_change, watch_project,
};

use crate::editor::EditorBuffer;
use crate::ui::Tab;

/// Font size that maps to zoom factor 1.0: the configured size scales the whole
/// interface, so there is one number to remember instead of two.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Limits of the interface zoom, the same egui puts on its own keyboard zoom.
pub const MIN_ZOOM: f32 = 0.2;
pub const MAX_ZOOM: f32 = 5.0;

/// How quiet the zoom keys have to be before the size is written to the config.
/// Holding `Ctrl`+`+` changes the factor every frame, and writing the file that
/// often would be a write storm for a value nobody reads mid-frame.
pub const ZOOM_SAVE_DELAY: f64 = 0.4;

/// Something the interface asked for, applied after the frame is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Open (or focus) a file, path relative to the project root.
    OpenFile(PathBuf),
    /// Open a file at a line and character (both 0-based), for diagnostics and
    /// navigation. A diagnostic has no column of its own, so it passes 0.
    OpenFileAt(PathBuf, u32, u32),
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
/// The blame of one file, kept until it is asked to go or the file changes.
#[derive(Clone, Debug)]
pub struct BlameView {
    /// The file the lines belong to.
    pub path: PathBuf,
    /// One entry per line, in file order.
    pub lines: Vec<ide_git::BlameLine>,
}

pub struct AppState {
    /// Whether the window's fonts have been handed to egui yet.
    ///
    /// They are set once, from the first frame: the fonts egui brings carry no `↳` and none of
    /// the arrows an editor wants, and the window puts DejaVu Sans behind them (see
    /// [`crate::ui::fonts_with_fallback`]).
    pub(crate) fonts_set: bool,
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
    /// The panels the person has put away, in the order they were put away. What is not here is
    /// on screen: a panel that is written down is a panel the dock does not hold.
    pub hidden: Vec<Tab>,
    /// A question waiting for an answer; the interface draws it as a modal.
    pub prompt: Option<Prompt>,
    /// Set when the user chose to leave; the next frame closes the window.
    pub quit_requested: bool,
    /// The user answered the quit question: the window may go, and a close that
    /// comes back as a request again must not be cancelled a second time.
    pub quit_decided: bool,
    /// Whether the Markdown preview sits beside the source.
    pub preview_open: bool,
    /// The quick-open field.
    pub quick_open: crate::ui::quick_open::QuickOpen,
    /// Where a symbol is used, once the server has answered.
    pub results: crate::ui::results::Results,
    /// The project-symbol search (`Ctrl+Shift+F12`).
    pub symbols: crate::ui::symbols::SymbolSearch,
    pub hover: crate::ui::hover::Hover,
    /// Whether the hints that follow the pointer are on: the switch lives in the
    /// View menu, and with it off the pointer is not watched at all.
    pub hints: bool,
    /// Whether the editor tab was drawn this frame: the hint belongs to the text under
    /// the pointer, and there is no such text while the panel is off screen.
    pub(crate) editor_drawn: bool,
    /// The blame of the active file, once asked for (`Ctrl+Shift+B`).
    pub blame: Option<BlameView>,
    pub rename: crate::ui::rename::Rename,
    /// The terminal tab: a shell on a pseudo-terminal.
    pub terminal: crate::ui::terminal::Terminal,
    /// The repository behind the project, and what it last said.
    pub git: crate::git::GitState,
    /// The conversation with an agent, and the worker that owns it.
    pub agent: crate::agent::AgentState,
    /// The language server of the project, once one has been needed.
    pub lsp: Option<crate::lsp::LanguageService>,
    /// Server name for the status bar, or why there is none.
    pub lsp_status: String,
    /// The caret whose uses were last asked for, and when it got there.
    highlight_key: Option<(PathBuf, usize)>,
    highlight_asked: Option<(PathBuf, usize)>,
    highlight_since: f64,
    /// A question about this caret came back without an answer, and when it was noticed. The
    /// question is put again, but only after a pause: a retry that follows the failure as fast
    /// as the frames come turns one slow answer into a flood of requests, and a server that is
    /// already behind answers none of them — the uses stay unmarked for as long as it is asked.
    highlight_failed: bool,
    highlight_failed_at: Option<f64>,
    /// When the zoom last changed, in input time, and the size last written.
    zoom_changed_at: f64,
    saved_font_size: f32,
    /// When documents were last sent to the server, in input time.
    pub last_sync: f64,
    /// Diagnostics per file, as the problems panel shows them.
    pub diagnostics: std::collections::BTreeMap<PathBuf, Vec<crate::lsp::DiagnosticLine>>,
    /// Layout cache of the Markdown preview.
    /// The find-and-replace bar of the editor.
    pub search: crate::ui::search::Search,
    pub preview: crate::ui::preview::Preview,
    pub commonmark: egui_commonmark::CommonMarkCache,
    /// Keeps the watch alive; dropping it stops the events.
    watcher: Option<ide_core::ProjectWatcher>,
    fs_events: Option<Receiver<FsEvent>>,
    next_buffer_id: usize,
    pub(crate) config_dir: PathBuf,
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
        let saved_font_size = config.font_size;

        let mut state = Self {
            tree: TreeState {
                show_hidden: config.show_hidden,
                ..TreeState::default()
            },
            config,
            fonts_set: false,
            recent,
            project: None,
            tabs: Tabs::new(),
            buffers: HashMap::new(),
            index: None,
            status: String::new(),
            pending: Vec::new(),
            layout: None,
            hidden: Vec::new(),
            prompt: None,
            quit_requested: false,
            quit_decided: false,
            preview_open: false,
            quick_open: crate::ui::quick_open::QuickOpen::new(),
            results: crate::ui::results::Results::new(),
            symbols: crate::ui::symbols::SymbolSearch::new(),
            hover: crate::ui::hover::Hover::new(),
            hints: true,
            editor_drawn: false,
            blame: None,
            rename: crate::ui::rename::Rename::new(),
            terminal: crate::ui::terminal::Terminal::new(),
            git: crate::git::GitState::default(),
            agent: crate::agent::AgentState::default(),
            lsp: None,
            lsp_status: String::new(),
            highlight_key: None,
            highlight_asked: None,
            highlight_since: 0.0,
            highlight_failed: false,
            highlight_failed_at: None,
            zoom_changed_at: 0.0,
            saved_font_size,
            last_sync: 0.0,
            diagnostics: std::collections::BTreeMap::new(),
            preview: crate::ui::preview::Preview::new(),
            commonmark: egui_commonmark::CommonMarkCache::default(),
            search: crate::ui::search::Search::default(),
            watcher: None,
            fs_events: None,
            next_buffer_id: 1,
            config_dir,
            workspace: None,
            index_job: None,
            last_state: None,
            last_write: 0.0,
        };

        // Launched without a project argument, the editor opens the one the user
        // had open last, the way every other IDE does: starting from an empty
        // window and picking the project again on every launch is busywork.
        let project = project.or_else(|| Self::last_project(&state.recent));
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

    /// The project to open when the command line named none.
    ///
    /// The newest remembered project that still exists wins. A directory that
    /// was moved or deleted since the last launch is skipped instead of being
    /// opened into an error, and the older entries get their chance.
    fn last_project(recent: &Recent) -> Option<PathBuf> {
        recent.list().iter().find(|path| path.is_dir()).cloned()
    }

    /// Opens a directory as the project: creates `.andy-ide`, shows the cached
    /// file index immediately and starts rebuilding it in the background.
    pub fn open_project(&mut self, root: &Path) {
        match Project::open(root) {
            Ok(project) => {
                let canonical = project.root().to_path_buf();
                // A shell opened from here belongs to this project: the terminal runs
                // `ls`, `git status` and builds, all of which answer about a directory.
                self.terminal.cwd = Some(canonical.clone());
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
        // A file of the project is held under its path relative to the root; a file
        // from outside it is held under its absolute path, and its own directory is
        // watched so that the event arrives at all.
        let rel = match absolute.strip_prefix(&root) {
            Ok(relative) => relative.to_path_buf(),
            Err(_) => absolute.clone(),
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

    /// Remembers the zoom of the whole interface, which egui changes itself on
    /// `Ctrl`+`+`, `Ctrl`+`-` and `Ctrl`+`0`.
    ///
    /// The value lives in `config.toml` as a font size, so there is one number
    /// for "how big everything is" instead of a zoom that multiplies a size.
    /// Holding a zoom key changes the factor every few frames; writing the file
    /// each time would be a write storm, so the write waits until the keys have
    /// been quiet for [`ZOOM_SAVE_DELAY`] and the returned duration says when the
    /// window should come back to do it. `None` means there is nothing left to
    /// write.
    pub fn remember_zoom(&mut self, zoom: f32, now: f64) -> Option<std::time::Duration> {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        // Two decimals is more than the tenths egui steps in, and keeps the
        // comparison from chasing the noise of `f32` multiplication.
        let size = (zoom * DEFAULT_FONT_SIZE * 100.0).round() / 100.0;
        if (size - self.config.font_size).abs() > 0.01 {
            self.config.font_size = size;
            self.zoom_changed_at = now;
        }
        if self.config.font_size == self.saved_font_size {
            return None;
        }
        let left = ZOOM_SAVE_DELAY - (now - self.zoom_changed_at);
        if left > 0.0 {
            return Some(std::time::Duration::from_secs_f64(left));
        }
        self.save_config();
        None
    }

    /// Asks the server what the pointer is resting on, once the pointer has
    /// rested long enough.
    ///
    /// Returns how long the window should wait before the question becomes due,
    /// so an idle window comes back to ask it; `None` means nothing is pending.
    pub fn track_hover(&mut self, now: f64) -> Option<std::time::Duration> {
        if !self.hints {
            // The hints are off: the pointer is not watched at all, and a hint that
            // was on screen when the switch went off is gone with it.
            self.hover.forget();
            return None;
        }
        self.hover.forget_if_modal(self.modal_open());
        // The pointer is on the hint, which floats over the text. The text under it is
        // not what the pointer is on, so nothing is asked about it: asking replaced the
        // answer being read with the answer about a symbol hidden behind the hint.
        // Looking at the spot is skipped too, since that is the first step of asking.
        if self.hover.is_over_hint() {
            self.hover.settle(now);
            return None;
        }

        let spot = self.hover_spot();
        self.hover.look_at(spot, now);
        // What is on screen is decided here, before it is drawn: a hint that the pointer
        // has left stays for the linger, and stays for good while the pointer is on it.
        self.hover.settle(now);

        let Some(spot) = self.hover.due(now) else {
            return self.hover.wait().or_else(|| self.hover.leaving_in(now));
        };
        if !self.start_server_for(&spot.path) {
            // Without a server there is nothing to ask; the spot is remembered as
            // answered so the question is not repeated every frame.
            self.hover.asked(spot);
            return None;
        }
        let Some(lsp) = &self.lsp else {
            return None;
        };
        lsp.send(crate::lsp::Job::Hover {
            path: spot.path.clone(),
            position: lsp_types::Position::new(spot.line, spot.character),
        });
        self.hover.asked(spot);
        None
    }

    /// Asks the server where the symbol under the caret is used in this file.
    ///
    /// Like the hint, this waits for the caret to stop: every key press moves it,
    /// and a request per press would queue in front of the next definition. A
    /// server that is not running yet is not started for decoration alone — the
    /// highlight appears once the editor has a reason to run a server.
    pub fn track_highlights(&mut self, now: f64) -> Option<std::time::Duration> {
        /// How long the caret stays put before its uses are asked for.
        const DELAY: f64 = 0.12;

        /// How long to wait before asking again when the question came back unanswered.
        const RETRY: f64 = 2.0;

        let Some(buffer) = self.active_buffer() else {
            self.highlight_key = None;
            self.highlight_asked = None;
            self.highlight_failed_at = None;
            return None;
        };
        let key = (buffer.path.clone(), buffer.caret_byte());
        if self.highlight_key.as_ref() != Some(&key) {
            self.highlight_key = Some(key);
            self.highlight_since = now;
            self.highlight_failed_at = None;
            return Some(std::time::Duration::from_secs_f64(DELAY));
        }
        // The failure is stamped here, where the clock is: an answer arrives from another
        // thread, and it has no business reading the time.
        if std::mem::take(&mut self.highlight_failed) {
            self.highlight_failed_at = Some(now);
        }
        if let Some(at) = self.highlight_failed_at {
            let waited = now - at;
            if waited < RETRY {
                return Some(std::time::Duration::from_secs_f64(RETRY - waited));
            }
            self.highlight_failed_at = None;
        }
        if self.highlight_asked.as_ref() == self.highlight_key.as_ref() {
            return None;
        }
        let waited = now - self.highlight_since;
        if waited < DELAY {
            return Some(std::time::Duration::from_secs_f64(DELAY - waited));
        }
        let (path, caret) = self.highlight_key.clone()?;
        let buffer = self.active_buffer()?;
        let position = ide_lsp::byte_to_position(buffer.text(), caret);
        let lsp = self.lsp.as_ref()?;
        lsp.send(crate::lsp::Job::Highlights { path, position });
        self.highlight_asked = self.highlight_key.clone();
        None
    }

    /// The uses of a symbol, as ranges of bytes in the file they were asked for.
    fn apply_highlights(
        &mut self,
        path: &Path,
        line: u32,
        character: u32,
        spots: &[(u32, u32, u32, u32)],
    ) {
        let Some(buffer) = self.active_buffer_mut() else {
            return;
        };
        if buffer.path != path {
            return;
        }
        // The caret may have moved while the server was thinking: the places of
        // the old symbol are not the places of the new one.
        let (caret_line, caret_col) = buffer.line_col();
        if caret_line != line as usize || caret_col != character as usize {
            return;
        }
        let text = buffer.text().to_owned();
        let ranges: Vec<(usize, usize)> = spots
            .iter()
            .map(|(start_line, start_col, end_line, end_col)| {
                let start = ide_lsp::position_to_byte(
                    &text,
                    lsp_types::Position::new(*start_line, *start_col),
                );
                let end =
                    ide_lsp::position_to_byte(&text, lsp_types::Position::new(*end_line, *end_col));
                (start, end.max(start))
            })
            .filter(|(start, end)| end > start)
            .collect();
        buffer.occurrences = ranges;
    }

    /// Opens the rename field for the word under the caret.
    pub fn begin_rename(&mut self) {
        let Some(buffer) = self.active_buffer() else {
            self.status = "open a file first".to_owned();
            return;
        };
        let text = buffer.text().to_owned();
        let path = buffer.path.clone();
        let caret = buffer.caret_byte();
        let Some((start, end)) = crate::rename::word_at(&text, caret) else {
            self.status = "no symbol under the caret to rename".to_owned();
            return;
        };
        if !self.start_server_for(&path) {
            return;
        }
        self.rename.begin(&text[start..end]);
    }

    /// Asks the server what renaming the symbol to `new_name` would change.
    pub fn submit_rename(&mut self) {
        let Some(buffer) = self.active_buffer() else {
            return;
        };
        let path = buffer.path.clone();
        let position = ide_lsp::byte_to_position(buffer.text(), buffer.caret_byte());
        let Some(lsp) = self.lsp.as_ref() else {
            self.rename.close();
            self.status = self.lsp_status.clone();
            return;
        };
        lsp.send(crate::lsp::Job::Rename {
            path,
            position,
            new_name: self.rename.name.trim().to_owned(),
        });
        self.status = format!("renaming to {}…", self.rename.name.trim());
    }

    /// The path a tab is opened under: relative inside the project, absolute
    /// outside it.
    ///
    /// A server answers with URIs, which are always absolute; tabs use the same
    /// relative paths the tree shows. Without this the editor would look for a
    /// buffer under a path no tab has, write the file behind it, and leave the
    /// open buffer showing the old name.
    fn tab_path(&self, path: &Path) -> PathBuf {
        match self.project.as_ref() {
            Some(project) => match path.strip_prefix(project.root()) {
                Ok(relative) => relative.to_path_buf(),
                Err(_) => path.to_path_buf(),
            },
            None => path.to_path_buf(),
        }
    }

    /// Writes the buffer that holds `path` to its file.
    fn save_buffer(&mut self, path: &Path) -> anyhow::Result<()> {
        let root = match self.project.as_ref() {
            Some(project) => project.root().to_path_buf(),
            None => PathBuf::new(),
        };
        let Some(buffer) = self.buffer_for_mut(path) else {
            return Ok(());
        };
        buffer.save(&root)?;
        self.sync_dirty();
        Ok(())
    }

    /// The text of a file as the editor sees it: an open buffer, or the disk.
    fn text_of(&self, path: &Path) -> Option<String> {
        if let Some(buffer) = self.buffer_for(path) {
            return Some(buffer.text().to_owned());
        }
        let absolute = match self.project.as_ref() {
            Some(project) => project.root().join(path),
            None => path.to_path_buf(),
        };
        std::fs::read_to_string(absolute).ok()
    }

    fn buffer_for(&self, path: &Path) -> Option<&EditorBuffer> {
        let id = self
            .tabs
            .items()
            .iter()
            .find(|item| item.path == path)
            .map(|item| item.buffer_id)?;
        self.buffers.get(&id)
    }

    fn buffer_for_mut(&mut self, path: &Path) -> Option<&mut EditorBuffer> {
        let id = self
            .tabs
            .items()
            .iter()
            .find(|item| item.path == path)
            .map(|item| item.buffer_id)?;
        self.buffers.get_mut(&id)
    }

    /// Turns the server's answer into spans of bytes, and shows the preview.
    fn preview_rename(&mut self, changes: crate::lsp::RenameChanges, new_name: &str) {
        let mut files = Vec::new();
        let mut measured = Vec::new();
        for (path, ranges) in changes {
            // The server speaks in URIs, the tabs in project-relative paths.
            let path = self.tab_path(&path);
            let Some(text) = self.text_of(&path) else {
                // A file the editor cannot read is a file it cannot rename.
                continue;
            };
            let spans: Vec<crate::ui::rename::Span> = ranges
                .iter()
                .map(|(start_line, start_col, end_line, end_col)| {
                    let start = ide_lsp::position_to_byte(
                        &text,
                        lsp_types::Position::new(*start_line, *start_col),
                    );
                    let end = ide_lsp::position_to_byte(
                        &text,
                        lsp_types::Position::new(*end_line, *end_col),
                    );
                    crate::ui::rename::Span {
                        line: crate::ui::rename::line_of(&text, start),
                        start,
                        end: end.max(start),
                    }
                })
                .collect();
            files.push(crate::ui::rename::FileChanges {
                path: path.clone(),
                spans,
            });
            measured.push((path, text));
        }
        if files.is_empty() {
            self.rename.close();
            self.status = "nothing to rename".to_owned();
            return;
        }
        self.rename.name = new_name.to_owned();
        self.rename.preview(files, measured);
    }

    /// Writes the rename the user has just looked at.
    pub fn apply_rename(&mut self) {
        let name = self.rename.name.trim().to_owned();
        let files = std::mem::take(&mut self.rename.changes);
        let measured = std::mem::take(&mut self.rename.measured);
        self.rename.close();
        if name.is_empty() || files.is_empty() {
            return;
        }

        // Nothing is written if any file changed since the preview: the list the
        // user accepted would no longer be the list of edits.
        for (path, text) in &measured {
            if self.text_of(path).as_deref() != Some(text.as_str()) {
                self.status = format!(
                    "{} changed since the preview: nothing renamed",
                    display_name(path)
                );
                return;
            }
        }

        let mut changed_files = 0;
        let mut changed_uses = 0;
        for file in &files {
            let Some(text) = measured
                .iter()
                .find(|(path, _)| *path == file.path)
                .map(|(_, text)| text.clone())
            else {
                continue;
            };
            let ranges: Vec<(usize, usize)> = file
                .spans
                .iter()
                .map(|span| (span.start, span.end))
                .collect();
            let Some(new_text) = crate::rename::apply_edits(&text, &ranges, &name) else {
                self.status = format!(
                    "{} has edits that do not fit its text",
                    display_name(&file.path)
                );
                return;
            };
            if new_text == text {
                continue;
            }
            changed_uses += file.spans.len();
            changed_files += 1;
            match self.buffer_for_mut(&file.path) {
                Some(buffer) => {
                    buffer.replace_text(new_text);
                    if let Err(error) = self.save_buffer(&file.path) {
                        self.status = format!(
                            "renamed, but {} was not saved: {error}",
                            display_name(&file.path)
                        );
                        return;
                    }
                }
                None => {
                    let absolute = match self.project.as_ref() {
                        Some(project) => project.root().join(&file.path),
                        None => file.path.clone(),
                    };
                    if let Err(error) = std::fs::write(&absolute, new_text) {
                        self.status = format!("cannot write {}: {error}", absolute.display());
                        return;
                    }
                }
            }
        }
        self.status = format!("renamed {changed_uses} uses in {changed_files} files to {name}");
    }

    /// Where the pointer rests, as the view of the active tab last saw it.
    ///
    /// Nothing while the editor is off screen: the spot is a place in a text under the
    /// pointer, and when that text is not drawn the place means nothing — a hint left
    /// over from an earlier frame follows the pointer into whatever panel is in use.
    pub(crate) fn hover_spot(&self) -> Option<crate::ui::hover::Spot> {
        if !self.editor_drawn {
            return None;
        }
        let buffer = self.active_buffer()?;
        let (line, character, screen) = buffer.hover?;
        Some(crate::ui::hover::Spot {
            path: buffer.path.clone(),
            line: line as u32,
            character: character as u32,
            screen,
        })
    }

    /// Puts the pointer on a symbol, as the view would.
    #[cfg(test)]
    pub(crate) fn set_hover_for_test(&mut self, line: u32, character: u32, screen: egui::Pos2) {
        if let Some(buffer) = self.active_buffer_mut() {
            buffer.hover = Some((line as usize, character as usize, screen));
        }
    }

    /// Whether something is on top of the editor that the pointer is really over.
    fn modal_open(&self) -> bool {
        self.results.open
            || self.symbols.open
            || self.quick_open.open
            || self.rename.open
            || self.prompt.is_some()
            || self.preview_open
    }

    /// Remembers whether the window is maximized.
    ///
    /// The window manager reports the state every frame and it changes only when
    /// the user maximizes or restores the window, so a change is written at once:
    /// there is nothing to debounce, and a size nobody wrote would be lost if the
    /// editor did not get a chance to close properly. `None` means this backend
    /// does not tell, and then the remembered value is left alone.
    pub fn remember_window(&mut self, maximized: Option<bool>) {
        let Some(maximized) = maximized else {
            return;
        };
        if self.config.window_maximized == maximized {
            return;
        }
        self.config.window_maximized = maximized;
        self.save_config();
    }

    /// Writes a zoom that is still waiting for its delay.
    ///
    /// Closing the window inside those four tenths of a second would otherwise
    /// take the last zoom with it: the frame that writes it never comes.
    pub fn flush_zoom(&mut self) {
        if self.config.font_size != self.saved_font_size {
            self.save_config();
        }
    }

    /// Writes the configuration back, so a theme change survives a restart.
    pub fn save_config(&mut self) {
        let path = self.config_dir.join("config.toml");
        match self.config.save(&path) {
            // The file now holds the zoom as well, so a pending write is done.
            Ok(()) => self.saved_font_size = self.config.font_size,
            Err(error) => self.status = format!("cannot save config: {error}"),
        }
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
        // What the panel knew about the context comes back with the session: an agent names the
        // size of its window only after the first exchange of a restored session, and a blank
        // figure until then reads as a session that remembered nothing.
        self.agent.kept_usage = state.context.map(|span| Usage {
            used: span.used,
            window: span.window,
        });
        for path in state.tabs {
            if root.join(&path).is_file() {
                self.open_file(&path);
                // The caret of the tab comes back with it: a tab restored to the top of the
                // file is not where the person was.
                if let Some((line, column)) = state.carets.get(&path).copied()
                    && let Some(index) = self.tabs.find(&path)
                    && let Some(item) = self.tabs.items().get(index)
                    && let Some(buffer) = self.buffers.get_mut(&item.buffer_id)
                {
                    let caret = buffer.char_index(line, column);
                    buffer.set_caret(caret);
                }
            }
        }
        if let Some(active) = state.active {
            // And the tab in front comes back with its caret on the screen, not merely in the
            // file.
            self.focus_tab(active);
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

    /// The panels this workspace remembers being put away, or `None` for a workspace that has
    /// never said. A panel this build does not know is left out rather than kept for a build
    /// that never comes.
    pub fn stored_hidden(&self) -> Option<Vec<Tab>> {
        let workspace = self.workspace.as_ref()?;
        let stored = workspace.read_state().hidden?;
        Some(
            stored
                .iter()
                .filter_map(|slug| Tab::of_slug(slug))
                .collect(),
        )
    }

    /// The View switches, as the workspace file keeps them: the two the window has now, under
    /// the names it knows them by.
    pub fn view_switches(&self) -> BTreeMap<String, bool> {
        BTreeMap::from([
            ("hidden-files".to_owned(), self.tree.show_hidden),
            ("preview".to_owned(), self.preview_open),
            ("hints".to_owned(), self.hints),
        ])
    }

    /// The View switches the project remembers, if it has ever said.
    pub fn stored_view(&self) -> Option<BTreeMap<String, bool>> {
        let workspace = self.workspace.as_ref()?;
        let view = workspace.read_state().view;
        (!view.is_empty()).then_some(view)
    }

    /// Puts the View switches of a project back. A name this build does not know is left alone:
    /// it belongs to a build that does.
    pub fn apply_view(&mut self, view: &BTreeMap<String, bool>) {
        if let Some(value) = view.get("hidden-files") {
            self.tree.show_hidden = *value;
        }
        if let Some(value) = view.get("preview") {
            self.preview_open = *value;
        }
        if let Some(value) = view.get("hints") {
            self.hints = *value;
        }
    }

    /// The agent that was on duty when the window closed, if the workspace remembers one.
    pub fn stored_agent(&self) -> Option<String> {
        let workspace = self.workspace.as_ref()?;
        workspace.read_state().agent
    }

    /// Puts a panel away: it leaves the dock, and everything behind it — a shell that is still
    /// running, a conversation, the changes of a repository — stays where it was.
    pub fn put_away(&mut self, tab: Tab) {
        if !self.hidden.contains(&tab) {
            self.hidden.push(tab);
            self.status = format!("{} put away", tab.title());
        }
    }

    /// Brings it back, and the workspace remembers that it is on screen.
    pub fn bring_back(&mut self, tab: Tab) {
        let was_hidden = self.hidden.len();
        self.hidden.retain(|hidden| *hidden != tab);
        if self.hidden.len() != was_hidden {
            self.status = format!("{} back on screen", tab.title());
        }
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
            hidden: Some(
                self.hidden
                    .iter()
                    .map(|tab| tab.slug().to_owned())
                    .collect(),
            ),
            // The agent that was on duty, so the next launch can open it as though the
            // person had pressed Start the moment the panel was in front of them. Only an
            // agent that has actually been opened counts: a fresh project has not chosen
            // one, and the panel's default name is not a duty to resume.
            agent: self.agent.has_connected().then(|| self.agent.name.clone()),
            // Where each open file was left. Taken from the buffers rather than from the tab in
            // front: every tab has its own caret, and every one of them is what a person comes
            // back to.
            carets: self
                .buffers
                .values()
                .map(|buffer| (buffer.path.clone(), buffer.line_col()))
                .collect(),
            // The View switches are part of the project as much as the panels are: a person who
            // works without hints does not want them back on the next launch.
            view: self.view_switches(),
            context: self.agent.context_usage().map(|usage| ContextSpan {
                used: usage.used,
                window: usage.window,
            }),
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

    /// Brings a tab to the front, and the view with it.
    ///
    /// Arriving at a tab is arriving at the caret: a caret left below the fold is a caret the
    /// person has to go looking for. Restoring the workspace and opening a file ask the same.
    pub fn focus_tab(&mut self, index: usize) {
        self.tabs.set_active(index);
        let buffer = self
            .tabs
            .items()
            .get(index)
            .and_then(|item| self.buffers.get_mut(&item.buffer_id));
        if let Some(buffer) = buffer {
            buffer.request_reveal();
        }
    }

    /// Opens a file in a tab and activates it, or focuses its existing tab.
    pub fn open_file(&mut self, rel: &Path) {
        if let Some(index) = self.tabs.find(rel) {
            self.focus_tab(index);
            self.status = display_name(rel);
            return;
        }
        // A file outside the project (a dependency's source, a definition in the
        // registry) is opened by its absolute path; everything below joins the
        // root, and joining an absolute path leaves it alone.
        let root = match self.project.as_ref() {
            Some(project) => project.root().to_path_buf(),
            None if rel.is_absolute() => PathBuf::new(),
            None => {
                self.status = "no project open".to_owned();
                return;
            }
        };
        match EditorBuffer::load(&root, rel) {
            Ok(buffer) => {
                let buffer_id = self.next_buffer_id;
                self.next_buffer_id += 1;
                self.buffers.insert(buffer_id, buffer);
                let index = self.tabs.open(rel.to_path_buf(), buffer_id);
                self.focus_tab(index);
                self.status = display_name(rel);
                // The project watch does not reach outside the root, so a file from
                // another crate gets its own directory watched; otherwise the editor
                // would show a stale copy forever.
                if rel.is_absolute()
                    && let (Some(watcher), Some(project)) =
                        (self.watcher.as_mut(), self.project.as_ref())
                    && !rel.starts_with(project.root())
                {
                    let _ = ide_core::watch_extra_file(watcher, rel);
                }
            }
            Err(error) => self.status = format!("{error:#}"),
        }
    }

    /// Starts the language server of the project once, if there is one installed.
    ///
    /// Called when a file is opened: a project of text files never pays for a
    /// server, and the handshake happens on the worker thread anyway.
    fn ensure_lsp(&mut self, rel: &Path) {
        if self.lsp.is_some() || !self.lsp_status.is_empty() {
            return;
        }
        let Some(server) = ide_lsp::server_for(rel) else {
            self.lsp_status = format!("no language server for {}", display_name(rel));
            return;
        };
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return;
        };
        if !ide_lsp::is_installed(server) {
            self.lsp_status = format!("{} is not installed", server.name);
            return;
        }
        self.lsp_status = format!("{} starting", server.name);
        self.lsp = Some(crate::lsp::LanguageService::start(server, &root));
    }

    /// Sends the current text of every open document that has unsaved edits.
    pub fn sync_open_documents(&mut self) {
        let Some(lsp) = self.lsp.as_ref() else {
            return;
        };
        let jobs: Vec<crate::lsp::Job> = self
            .tabs
            .items()
            .iter()
            .filter_map(|item| {
                let buffer = self.buffers.get(&item.buffer_id)?;
                if !buffer.is_dirty() {
                    return None;
                }
                Some(crate::lsp::Job::Change {
                    path: item.path.clone(),
                    text: buffer.text().to_owned(),
                })
            })
            .collect();
        for job in jobs {
            lsp.send(job);
        }
    }

    /// Asks the server where the symbol under the caret is defined.
    ///
    /// The server is started here rather than when a project opens: a project
    /// of text files never pays for one, and a cold rust-analyzer costs a second
    /// or two, which is why it happens on the worker thread while the editor
    /// keeps drawing.
    pub fn goto_definition(&mut self) {
        self.navigate(false);
    }

    /// Asks the server for every place the symbol under the caret is used.
    pub fn find_references(&mut self) {
        self.navigate(true);
    }

    /// Shows the structure of the file in the active tab.
    pub fn file_symbols(&mut self) {
        let Some(buffer) = self.active_buffer() else {
            return;
        };
        let path = buffer.path.clone();
        if !self.start_server_for(&path) {
            return;
        }
        if let Some(lsp) = self.lsp.as_ref() {
            self.status = "looking for symbols...".to_owned();
            lsp.send(crate::lsp::Job::Symbols { path });
        }
    }

    /// Opens the project-symbol search (`Ctrl+Shift+F12`).
    ///
    /// The server is the one that understands the file in front of the user: a
    /// project with two languages has two servers, and asking the wrong one for
    /// workspace symbols answers nothing useful.
    pub fn open_symbol_search(&mut self) {
        let path = self
            .active_buffer()
            .map(|buffer| buffer.path.clone())
            .or_else(|| self.tabs.items().first().map(|item| item.path.clone()));
        let Some(path) = path else {
            self.status = "open a file first: its language picks the server".to_owned();
            return;
        };
        if !self.start_server_for(&path) {
            return;
        }
        self.symbols.open();
        self.status = "type a name: the server searches the project".to_owned();
    }

    /// Sends the query the search has settled on, and asks for the frame that
    /// will be needed when the debounce runs out.
    ///
    /// Called once per frame; `now` is the egui clock. The debounce lives in
    /// [`crate::ui::symbols::SymbolSearch`], so a fast typist costs one request
    /// per pause rather than one per letter.
    pub fn poll_symbol_search(&mut self, now: f64) {
        if !self.symbols.open {
            return;
        }
        if let Some(query) = self.symbols.due(now)
            && let Some(lsp) = self.lsp.as_ref()
        {
            lsp.send(crate::lsp::Job::WorkspaceSymbols { query });
        }
    }

    /// Starts the server for `path` and announces the open documents.
    ///
    /// Returns whether there is a server to ask; when there is not, the status
    /// line already says why.
    fn start_server_for(&mut self, path: &Path) -> bool {
        self.ensure_lsp(path);
        if self.lsp.is_none() {
            self.status = self.lsp_status.clone();
            return false;
        }
        // Whatever is already open has to be announced: the server was not there
        // when those tabs were opened.
        self.replay_documents();
        true
    }

    /// Sends a navigation request for the symbol under the caret.
    ///
    /// `references` picks the wider question; the rest of the way is the same,
    /// down to starting the server and announcing the open documents.
    fn navigate(&mut self, references: bool) {
        let Some(buffer) = self.active_buffer() else {
            return;
        };
        let path = buffer.path.clone();
        let position = ide_lsp::byte_to_position(buffer.text(), buffer.caret_byte());

        if !self.start_server_for(&path) {
            return;
        }
        if let Some(lsp) = self.lsp.as_ref() {
            self.status = if references {
                "looking for references...".to_owned()
            } else {
                "looking for the definition...".to_owned()
            };
            lsp.send(if references {
                crate::lsp::Job::References { path, position }
            } else {
                crate::lsp::Job::Definition { path, position }
            });
        }
    }

    /// Takes a navigation request left by a Ctrl+Click in the editor.
    pub fn poll_navigation(&mut self) {
        let Some(buffer_id) = self
            .tabs
            .active()
            .and_then(|index| self.tabs.items().get(index))
            .map(|item| item.buffer_id)
        else {
            return;
        };
        // Requests are drained from every buffer: one left in a tab the user
        // switched away from must not fire when they come back to it.
        let mut pending = None;
        for (id, buffer) in self.buffers.iter_mut() {
            if let Some(index) = buffer.take_nav_request()
                && *id == buffer_id
            {
                pending = Some(index);
            }
        }
        let Some(index) = pending else {
            return;
        };
        if let Some(buffer) = self.buffers.get_mut(&buffer_id) {
            buffer.set_caret(index);
        }
        self.goto_definition();
    }

    /// Turns the server's locations into rows of the list.
    ///
    /// Paths are made relative to the project (anything outside it is dropped, as
    /// it cannot be opened here), duplicates are merged, and the rows are ordered
    /// by file and line so the list reads like a search result.
    fn hits_from(&self, locations: Vec<lsp_types::Location>) -> Vec<crate::ui::results::Hit> {
        use crate::ui::results::Hit;

        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return Vec::new();
        };
        let mut hits: Vec<Hit> = Vec::new();
        for location in locations {
            let Some(absolute) = ide_lsp::uri_to_path(&location.uri) else {
                continue;
            };
            let Ok(rel) = absolute.strip_prefix(&root) else {
                continue;
            };
            let hit = Hit::reference(
                rel.to_path_buf(),
                location.range.start.line,
                location.range.start.character,
                self.line_at(rel, location.range.start.line),
            );
            if hits.iter().any(|known| {
                known.path == hit.path && known.line == hit.line && known.character == hit.character
            }) {
                continue;
            }
            hits.push(hit);
        }
        hits.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.line.cmp(&right.line))
                .then(left.character.cmp(&right.character))
        });
        hits
    }

    /// Flattens the server's nested outline into rows, parents before children.
    ///
    /// The list is a flat `selectable_label` per row, so the tree is carried by
    /// the indentation of `depth` rather than by collapsing headers: an outline
    /// of thirty rows is easier to scan than a tree that has to be opened.
    fn outline_of(
        &self,
        from: &Path,
        symbols: &[lsp_types::DocumentSymbol],
    ) -> Vec<crate::ui::results::Hit> {
        use crate::ui::results::{Hit, SymbolKind};

        fn walk(
            symbols: &[lsp_types::DocumentSymbol],
            path: &Path,
            depth: usize,
            rows: &mut Vec<Hit>,
        ) {
            for symbol in symbols {
                rows.push(Hit::symbol(
                    path.to_path_buf(),
                    symbol.selection_range.start.line,
                    symbol.selection_range.start.character,
                    symbol.name.clone(),
                    depth,
                    SymbolKind::from_lsp(symbol.kind),
                ));
                if let Some(children) = &symbol.children {
                    walk(children, path, depth + 1, rows);
                }
            }
        }

        let mut rows = Vec::new();
        walk(symbols, from, 0, &mut rows);
        rows
    }

    /// Turns the server's flat list of project symbols into rows.
    ///
    /// A symbol outside the project keeps its absolute path so that the list can
    /// still open it, the same way a definition outside the project is opened.
    fn workspace_hits(
        &self,
        symbols: Vec<lsp_types::SymbolInformation>,
    ) -> Vec<crate::ui::results::Hit> {
        use crate::ui::results::{Hit, SymbolKind};

        let root = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf());
        let mut rows: Vec<Hit> = Vec::new();
        for symbol in symbols {
            let Some(absolute) = ide_lsp::uri_to_path(&symbol.location.uri) else {
                continue;
            };
            let path = match &root {
                Some(root) => match absolute.strip_prefix(root) {
                    Ok(rel) => rel.to_path_buf(),
                    Err(_) => absolute.clone(),
                },
                None => absolute.clone(),
            };
            rows.push(Hit::symbol(
                path,
                symbol.location.range.start.line,
                symbol.location.range.start.character,
                symbol.name,
                0,
                SymbolKind::from_lsp(symbol.kind),
            ));
        }
        rows.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.line.cmp(&right.line))
                .then(left.character.cmp(&right.character))
        });
        rows
    }

    /// The text of one line: from the buffer when the file is open (it may hold
    /// edits), from disk otherwise.
    fn line_at(&self, rel: &Path, line: u32) -> String {
        let open = self
            .tabs
            .items()
            .iter()
            .find(|item| item.path == rel)
            .and_then(|item| self.buffers.get(&item.buffer_id));
        if let Some(buffer) = open {
            return buffer.line_text(line as usize).to_owned();
        }
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            return String::new();
        };
        std::fs::read_to_string(root.join(rel))
            .ok()
            .and_then(|text| text.lines().nth(line as usize).map(str::to_owned))
            .unwrap_or_default()
    }

    /// Tells the server about every open document that it can understand.
    fn replay_documents(&mut self) {
        let Some(lsp) = self.lsp.as_ref() else {
            return;
        };
        let jobs: Vec<crate::lsp::Job> = self
            .tabs
            .items()
            .iter()
            .filter(|item| ide_lsp::server_for(&item.path).is_some())
            .filter_map(|item| {
                let buffer = self.buffers.get(&item.buffer_id)?;
                Some(crate::lsp::Job::Open {
                    path: item.path.clone(),
                    text: buffer.text().to_owned(),
                })
            })
            .collect();
        for job in jobs {
            lsp.send(job);
        }
    }

    /// Takes what the language server answered since the last frame.
    pub fn poll_lsp(&mut self) {
        let Some(lsp) = self.lsp.as_ref() else {
            return;
        };
        let outcomes = lsp.poll();
        for outcome in outcomes {
            self.apply_outcome(outcome);
        }
    }

    pub(crate) fn apply_outcome(&mut self, outcome: crate::lsp::Outcome) {
        match outcome {
            crate::lsp::Outcome::Failed(message) => {
                self.status = message;
                // A failed request leaves the field open with no answer coming:
                // the status line says why, and the modal gets out of the way.
                self.rename.close();
            }
            crate::lsp::Outcome::Hover {
                path,
                line,
                character,
                text,
            } => self.hover.answer(&path, line, character, text),
            crate::lsp::Outcome::Highlights {
                path,
                line,
                character,
                spots,
                failed,
            } => {
                if failed {
                    // The question is put again — after a pause, which `track_highlights` keeps:
                    // the server was busy, which is not the same as "this symbol is used
                    // nowhere", and asking again at once is what keeps it busy.
                    self.highlight_asked = None;
                    self.highlight_failed = true;
                } else {
                    self.highlight_failed_at = None;
                }
                self.apply_highlights(&path, line, character, &spots)
            }
            crate::lsp::Outcome::Rename { new_name, changes } => {
                self.preview_rename(changes, &new_name)
            }
            crate::lsp::Outcome::NoDefinition { from } => {
                self.status = format!("no definition for the symbol in {}", display_name(&from));
            }
            crate::lsp::Outcome::Symbols { from, symbols } => {
                let hits = self.outline_of(&from, &symbols);
                let count = hits.len();
                if count == 0 {
                    self.status = format!("no symbols in {}", display_name(&from));
                    return;
                }
                self.results
                    .show_symbols(format!("Symbols in {}: {count}", display_name(&from)), hits);
                self.status = format!("{count} symbols in {}", display_name(&from));
            }
            crate::lsp::Outcome::WorkspaceSymbols { query, symbols } => {
                let hits = self.workspace_hits(symbols);
                let count = hits.len();
                // The answer carries its question: a slow server may answer a
                // query the user has already typed past, and that answer must
                // not replace what is on screen.
                self.symbols.apply(&query, hits);
                if self.symbols.open {
                    self.status = format!("{count} symbols match «{query}»");
                }
            }
            crate::lsp::Outcome::References { from, locations } => {
                let hits = self.hits_from(locations);
                let count = hits.len();
                self.results.show_hits(hits);
                self.status = if count == 0 {
                    format!("no references to the symbol in {}", display_name(&from))
                } else {
                    format!("{count} references")
                };
            }
            crate::lsp::Outcome::Definitions { from, locations } => {
                let Some(first) = locations.first() else {
                    return;
                };
                let Some(absolute) = ide_lsp::uri_to_path(&first.uri) else {
                    self.status = "the definition is not a file".to_owned();
                    return;
                };
                let root = self
                    .project
                    .as_ref()
                    .map(|project| project.root().to_path_buf());
                // A definition outside the project (a crate in the registry, a
                // dependency of the workspace) is opened all the same: it is
                // the code the user asked to see, and the tab keeps the absolute
                // path, which is what reading and saving use.
                // Inside the project the tab keeps a relative path, like every other
                // tab; outside it keeps the absolute one, which is what reading and
                // saving use.
                let rel = match root
                    .as_ref()
                    .and_then(|root| absolute.strip_prefix(root).ok())
                {
                    Some(rel) => rel.to_path_buf(),
                    None => absolute.clone(),
                };
                let external = rel.is_absolute();
                let text = std::fs::read_to_string(&absolute).unwrap_or_default();
                let byte = ide_lsp::position_to_byte(&text, first.range.start);
                self.open_file(&rel);
                self.place_caret(&rel, byte);
                self.status = if external {
                    format!(
                        "{}:{}:{} (outside the project)",
                        absolute.display(),
                        first.range.start.line + 1,
                        first.range.start.character + 1
                    )
                } else if locations.len() > 1 {
                    format!(
                        "{} definitions, jumped to {}:{}",
                        locations.len(),
                        display_name(&rel),
                        first.range.start.line + 1
                    )
                } else {
                    format!(
                        "{}:{}:{}",
                        display_name(&rel),
                        first.range.start.line + 1,
                        first.range.start.character + 1
                    )
                };
                let _ = from;
            }
            crate::lsp::Outcome::Diagnostics { path, lines } => {
                let rel = self
                    .project
                    .as_ref()
                    .and_then(|project| path.strip_prefix(project.root()).ok())
                    .map(Path::to_path_buf)
                    .unwrap_or(path);
                if lines.is_empty() {
                    self.diagnostics.remove(&rel);
                } else {
                    self.diagnostics.insert(rel, lines);
                }
            }
        }
    }

    /// Opens a file a terminal link named, at the line it named.
    ///
    /// A compiler prints `src/main.rs:12:5`, and that is where the user wants to be:
    /// the tab opens on the line, not at the top of the file.
    pub fn follow_file_link(&mut self, path: &str, line: u32, column: u32) {
        let named = PathBuf::from(path);
        let root = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf());
        let absolute = match (&root, named.is_absolute()) {
            (_, true) => named.clone(),
            (Some(root), false) => root.join(&named),
            (None, false) => {
                self.status = format!("no project: cannot open {path}");
                return;
            }
        };
        let text = match std::fs::read_to_string(&absolute) {
            Ok(text) => text,
            Err(error) => {
                self.status = format!("{}: {error}", absolute.display());
                return;
            }
        };
        let rel = match root
            .as_ref()
            .and_then(|root| absolute.strip_prefix(root).ok())
        {
            Some(rel) => rel.to_path_buf(),
            None => absolute.clone(),
        };
        let position = lsp_types::Position::new(line.saturating_sub(1), column.saturating_sub(1));
        let byte = ide_lsp::position_to_byte(&text, position);
        self.open_file(&rel);
        self.place_caret(&rel, byte);
        self.status = format!("{}:{}:{}", display_name(&rel), line, column);
    }

    /// Puts the caret at a byte offset of a tab, opening it first if needed.
    pub(crate) fn place_caret(&mut self, rel: &Path, byte: usize) {
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
            buffer.set_caret_byte(byte);
            // The view has to show what the jump was for.
            buffer.request_reveal();
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

    /// Shows or hides the blame of the active file.
    ///
    /// The whole file is blamed once and kept: asking about another line of the same
    /// file costs nothing, and asking again for the blame after it has been shown is a
    /// request to hide it.
    pub fn toggle_blame(&mut self) {
        if self.blame.is_some() {
            self.blame = None;
            self.status = "blame hidden".to_owned();
            return;
        }
        let Some(path) = self.active_buffer().map(|buffer| buffer.path.clone()) else {
            self.status = "nothing to blame: no file is open".to_owned();
            return;
        };
        let Some(relative) = self.git.repo_relative(&path) else {
            self.status = "nothing to blame: this project is not a repository".to_owned();
            return;
        };
        let Some(repo) = &self.git.repo else {
            self.status = "nothing to blame: this project is not a repository".to_owned();
            return;
        };
        match repo.blame(&relative) {
            Ok(lines) if lines.is_empty() => {
                self.blame = None;
                self.status = "nothing to blame: the file is not committed".to_owned();
            }
            Ok(lines) => {
                self.blame = Some(BlameView { path, lines });
                self.status = "blame shown".to_owned();
            }
            Err(error) => {
                self.blame = None;
                self.status = format!("{error}");
            }
        }
    }

    /// The blame of the line the caret is on, when it has been asked for.
    pub fn blame_at_caret(&self) -> Option<&ide_git::BlameLine> {
        let view = self.blame.as_ref()?;
        let (line, _) = self.caret_position()?;
        // The blame describes the committed file: once the buffer has been edited the
        // lines have moved and the answer would point at the wrong commits.
        if self
            .tabs
            .active()
            .is_some_and(|index| self.tabs.dirty(index))
        {
            return None;
        }
        if self.active_buffer().map(|buffer| &buffer.path) != Some(&view.path) {
            return None;
        }
        view.lines.get(line.checked_sub(1)?)
    }

    /// Name of the active buffer.s language, if it has one.
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
        let item = self.tabs.items().get(index).map(|item| item.buffer_id);
        let path = self.tabs.items().get(index).map(|item| item.path.clone());
        self.prompt = None;
        if self.tabs.close(index) {
            if let Some(buffer_id) = item {
                self.buffers.remove(&buffer_id);
            }
            // The server stops tracking a file the editor no longer shows.
            if let (Some(lsp), Some(path)) = (self.lsp.as_ref(), path)
                && ide_lsp::server_for(&path).is_some()
            {
                lsp.send(crate::lsp::Job::Close { path });
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
                Action::OpenFileAt(rel, line, character) => {
                    self.open_file(&rel);
                    // The server counts characters in UTF-16 units, so the column
                    // is turned into a byte offset the same way a request is.
                    let byte = self
                        .active_buffer()
                        .map(|buffer| {
                            ide_lsp::position_to_byte(
                                buffer.text(),
                                lsp_types::Position { line, character },
                            )
                        })
                        .unwrap_or(0);
                    self.place_caret(&rel, byte);
                }
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
                    self.quit_decided = true;
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

/// The label of one tab, with the tail of the path for a file outside the project.
///
/// Only used when two tabs would otherwise read the same; `tab_labels` decides.
///
/// Inside the project the file name is enough — the tree says where the file is. A
/// file from outside it keeps the tail of its path instead: `mod.rs` appears in a
/// dependency's every directory, and two tabs reading `mod.rs` tell the user nothing
/// about which one they are looking at.
pub fn tab_title(path: &Path, root: Option<&Path>) -> String {
    if !path.is_absolute() || root.is_some_and(|root| path.starts_with(root)) {
        return display_name(path);
    }
    let parts: Vec<String> = path
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.len() <= 3 {
        return format!("/{}", parts.join("/"));
    }
    parts[parts.len() - 3..].join("/")
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

/// What opens when nobody said which project to open.
#[cfg(test)]
mod launch_tests {
    use super::*;
    use ide_core::Recent;

    fn remembered(config: &Path, projects: &[&Path]) {
        let mut recent = Recent::default();
        // `add` puts its argument in front, so the caller lists the projects in
        // the order they were opened: the last one ends up newest.
        for project in projects {
            recent.add(project.to_path_buf());
        }
        recent.save(&config.join("recent.json")).unwrap();
    }

    #[test]
    fn without_a_project_argument_the_last_one_opens() {
        let config = tempfile::tempdir().unwrap();
        let older = tempfile::tempdir().unwrap();
        let newer = tempfile::tempdir().unwrap();
        remembered(config.path(), &[older.path(), newer.path()]);

        let state = AppState::with_config_dir(config.path().to_path_buf(), None, None);

        let opened = state
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf());
        assert_eq!(
            opened.as_deref(),
            Some(newer.path().canonicalize().unwrap().as_path()),
            "the last project did not open by itself"
        );
    }

    #[test]
    fn a_project_that_is_gone_is_skipped() {
        let config = tempfile::tempdir().unwrap();
        let alive = tempfile::tempdir().unwrap();
        let gone = config.path().join("gone");
        std::fs::create_dir(&gone).unwrap();
        remembered(config.path(), &[alive.path(), &gone]);
        std::fs::remove_dir(&gone).unwrap();

        let state = AppState::with_config_dir(config.path().to_path_buf(), None, None);

        let opened = state
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf());
        assert_eq!(
            opened.as_deref(),
            Some(alive.path().canonicalize().unwrap().as_path()),
            "a deleted project was opened or the older one was not tried"
        );
    }

    #[test]
    fn an_explicit_project_wins_over_the_remembered_one() {
        let config = tempfile::tempdir().unwrap();
        let remembered_project = tempfile::tempdir().unwrap();
        let asked = tempfile::tempdir().unwrap();
        remembered(config.path(), &[remembered_project.path()]);

        let state = AppState::with_config_dir(
            config.path().to_path_buf(),
            Some(asked.path().to_path_buf()),
            None,
        );

        let opened = state
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf());
        assert_eq!(
            opened.as_deref(),
            Some(asked.path().canonicalize().unwrap().as_path()),
            "the remembered project overrode the one on the command line"
        );
    }

    #[test]
    fn a_first_launch_starts_with_no_project() {
        let config = tempfile::tempdir().unwrap();
        let state = AppState::with_config_dir(config.path().to_path_buf(), None, None);
        assert!(state.project.is_none(), "a project appeared from nowhere");
    }
}

/// The zoom of the interface, remembered between launches.
#[cfg(test)]
mod zoom_tests {
    use super::*;

    fn config_text(config: &Path) -> String {
        std::fs::read_to_string(config.join("config.toml")).unwrap_or_default()
    }

    #[test]
    fn a_zoom_change_is_written_after_the_delay() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::with_config_dir(dir.path().to_path_buf(), None, None);
        assert_eq!(state.config.font_size, DEFAULT_FONT_SIZE);

        let wait = state.remember_zoom(1.3, 10.0);
        assert!(wait.is_some(), "the write was not scheduled");
        assert_eq!(state.config.font_size, 18.2);
        assert!(
            !config_text(dir.path()).contains("font_size"),
            "the file was written before the delay ran out"
        );

        // The frame that comes after the delay does the write.
        let later = 10.0 + ZOOM_SAVE_DELAY + 0.01;
        assert!(state.remember_zoom(1.3, later).is_none());
        assert!(
            config_text(dir.path()).contains("font_size = 18.2"),
            "the zoom was not written: {}",
            config_text(dir.path())
        );
    }

    #[test]
    fn a_second_change_moves_the_write_forward() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::with_config_dir(dir.path().to_path_buf(), None, None);

        state.remember_zoom(1.1, 0.0);
        // A key pressed again before the delay ran out re-arms the wait: the
        // file is written once the keys have been quiet, not on a fixed clock.
        let second = ZOOM_SAVE_DELAY / 2.0;
        let wait = state.remember_zoom(1.2, second);
        assert_eq!(
            wait,
            Some(std::time::Duration::from_secs_f64(ZOOM_SAVE_DELAY)),
            "the wait was not counted from the last change"
        );
        assert_eq!(state.config.font_size, 16.8);
        assert!(
            !config_text(dir.path()).contains("font_size"),
            "the file was written before the keys went quiet"
        );

        assert!(
            state
                .remember_zoom(1.2, second + ZOOM_SAVE_DELAY + 0.001)
                .is_none()
        );
        assert!(config_text(dir.path()).contains("font_size = 16.8"));
    }

    #[test]
    fn the_same_zoom_asks_for_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::with_config_dir(dir.path().to_path_buf(), None, None);
        // The zoom of the first frame is the configured one, not a change.
        assert!(state.remember_zoom(1.0, 0.0).is_none());
        assert!(state.remember_zoom(1.0, 5.0).is_none());
        assert_eq!(state.config.font_size, DEFAULT_FONT_SIZE);
        assert!(!config_text(dir.path()).contains("font_size"));
    }

    #[test]
    fn a_saved_zoom_comes_back_on_the_next_launch() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut state = AppState::with_config_dir(dir.path().to_path_buf(), None, None);
            state.remember_zoom(1.5, 0.0);
            assert!(state.remember_zoom(1.5, ZOOM_SAVE_DELAY + 0.01).is_none());
        }
        let again = AppState::with_config_dir(dir.path().to_path_buf(), None, None);
        assert_eq!(again.config.font_size, 21.0);
    }

    #[test]
    fn the_saved_zoom_stays_within_what_the_window_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::with_config_dir(dir.path().to_path_buf(), None, None);
        state.remember_zoom(9.0, 0.0);
        assert_eq!(state.config.font_size, MAX_ZOOM * DEFAULT_FONT_SIZE);
        state.remember_zoom(0.01, 1.0);
        assert_eq!(state.config.font_size, MIN_ZOOM * DEFAULT_FONT_SIZE);
    }
}

/// The labels of the tab strip.
///
/// A file inside the project is named by its file name — the tree says where it is.
/// Outside it, the name alone is often enough, but `mod.rs` lives in every directory
/// of a dependency, so a name that two tabs share is replaced by the tail of the path.
pub fn tab_labels(paths: &[PathBuf], root: Option<&Path>) -> Vec<String> {
    let names: Vec<String> = paths.iter().map(|path| display_name(path)).collect();
    let mut labels = names.clone();
    for (index, name) in names.iter().enumerate() {
        if names.iter().filter(|other| *other == name).count() > 1 {
            labels[index] = tab_title(&paths[index], root);
        }
    }
    labels
}
