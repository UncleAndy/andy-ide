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
    /// The language server of the project, once one has been needed.
    pub lsp: Option<crate::lsp::LanguageService>,
    /// Server name for the status bar, or why there is none.
    pub lsp_status: String,
    /// When documents were last sent to the server, in input time.
    pub last_sync: f64,
    /// Diagnostics per file, as the problems panel shows them.
    pub diagnostics: std::collections::BTreeMap<PathBuf, Vec<crate::lsp::DiagnosticLine>>,
    /// Layout cache of the Markdown preview.
    pub preview: crate::ui::preview::Preview,
    pub commonmark: egui_commonmark::CommonMarkCache,
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
            quit_decided: false,
            preview_open: false,
            quick_open: crate::ui::quick_open::QuickOpen::new(),
            results: crate::ui::results::Results::new(),
            symbols: crate::ui::symbols::SymbolSearch::new(),
            lsp: None,
            lsp_status: String::new(),
            last_sync: 0.0,
            diagnostics: std::collections::BTreeMap::new(),
            preview: crate::ui::preview::Preview::new(),
            commonmark: egui_commonmark::CommonMarkCache::default(),
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

    /// Writes the configuration back, so a theme change survives a restart.
    pub fn save_config(&mut self) {
        let path = self.config_dir.join("config.toml");
        if let Err(error) = self.config.save(&path) {
            self.status = format!("cannot save config: {error}");
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
                self.tabs.set_active(index);
                self.status = display_name(rel);
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
            crate::lsp::Outcome::Failed(message) => self.status = message,
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

    /// Puts the caret at a byte offset of a tab, opening it first if needed.
    fn place_caret(&mut self, rel: &Path, byte: usize) {
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
