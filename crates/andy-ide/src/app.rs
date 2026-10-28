//! The application: dock layout, menu bar, status bar.

use std::path::PathBuf;

use egui::{Key, KeyboardShortcut, Modifiers};
use egui_dock::{DockArea, DockState, Node, NodePath, Split, Style, TabInsert};

use crate::state::{AppState, DEFAULT_FONT_SIZE, MAX_ZOOM, MIN_ZOOM, display_name};
use crate::ui::{Tab, Viewer};

/// How often the dock layout is turned into JSON while the app runs.
const LAYOUT_SERIALIZE_SECS: f64 = 0.5;

/// The window's contents.
pub struct App {
    pub(crate) state: AppState,
    pub(crate) dock: DockState<Tab>,
    zoom_applied: bool,
    /// Frames drawn so far; the dock has no real rectangles before the first one.
    frames: u32,
    /// When the dock layout was last serialized, in input time.
    layout_serialized_at: f64,
}

impl App {
    pub fn new(project: Option<PathBuf>, file: Option<PathBuf>) -> Self {
        Self::with_config_dir(crate::state::config_dir(), project, file)
    }

    /// Same as [`App::new`] with an explicit configuration directory.
    pub fn with_config_dir(
        config_dir: PathBuf,
        project: Option<PathBuf>,
        file: Option<PathBuf>,
    ) -> Self {
        let state = AppState::with_config_dir(config_dir, project, file);
        let dock = with_terminal(
            state
                .stored_layout()
                .and_then(|layout| serde_json::from_str::<DockState<Tab>>(&layout).ok())
                .unwrap_or_else(default_layout),
        );
        Self {
            state,
            dock,
            zoom_applied: false,
            frames: 0,
            layout_serialized_at: 0.0,
        }
    }
}

/// Editor in the middle, projects on the left, problems along the bottom.
///
/// In a split, `fraction` is the share taken by the top (or left) child. With
/// `Split::Left` the new panel goes left, so it gets the small fraction; with
/// `Split::Below` the new panel goes bottom, so the fraction is the editor's share.
fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Editor]);
    dock.split(
        NodePath::MAIN_ROOT,
        Split::Left,
        0.22,
        Node::leaf(Tab::Projects),
    );
    dock.split(
        NodePath::MAIN_ROOT,
        Split::Below,
        0.72,
        Node::leaf(Tab::Problems),
    );
    with_terminal(dock)
}

/// Puts the terminal in the bottom leaf as a tab beside the problems.
///
/// The bottom strip is one row of tabs, not two panes: a pane of its own took half
/// the width of the terminal away from the code above and half of it from the panel
/// beside it. Both panels are drawn by their own tab, so whichever is in front uses
/// the whole width.
///
/// A saved layout can disagree with that in two ways, and both are tidied here: it
/// may have no terminal tab at all, or it may have one in a pane of its own from a
/// version that split the strip.
fn with_terminal(dock: DockState<Tab>) -> DockState<Tab> {
    let dock = place_beside_problems(place_beside_problems(dock, Tab::Terminal), Tab::Git);
    place_beside_problems(dock, Tab::Diff)
}

/// Puts `tab` in the bottom leaf, beside the problems, where it belongs.
///
/// A saved layout can disagree with that in two ways, and both are tidied here: it
/// may have no such tab at all, or it may have one in a pane of its own from a
/// version that split the strip.
fn place_beside_problems(mut dock: DockState<Tab>, tab: Tab) -> DockState<Tab> {
    let Some(problems) = dock.find_tab(&Tab::Problems) else {
        if dock.find_tab(&tab).is_none() {
            dock.push_to_first_leaf(tab);
        }
        return dock;
    };
    match dock.find_tab(&tab) {
        Some(found) if found.node_path() == problems.node_path() => dock,
        Some(found) => {
            dock.move_tab(found, (problems.node_path(), TabInsert::Append));
            // The problems were in front before the move and are in front after it.
            let _ = dock.set_active_tab(problems);
            dock
        }
        None => {
            // A new tab would otherwise land in the focused leaf, which is not
            // necessarily the bottom one.
            dock.set_focused_node_and_surface(problems.node_path());
            dock.push_to_focused_leaf(tab);
            let _ = dock.set_active_tab(problems);
            dock
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl App {
    /// One frame. Kept separate from the `eframe::App` impl so it can be driven
    /// by a headless test.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        if !self.zoom_applied {
            self.zoom_applied = true;
            let zoom = self.state.config.font_size / DEFAULT_FONT_SIZE;
            ui.ctx().set_zoom_factor(zoom.clamp(MIN_ZOOM, MAX_ZOOM));
        }

        // egui changes the zoom itself on `Ctrl`+`+`, `Ctrl`+`-` and `Ctrl`+`0`,
        // so the window reads it back every frame instead of keeping a number of
        // its own. A change is written to the config once the keys go quiet, and
        // the frame that will do it is asked for here: an idle window would
        // otherwise never come back and the zoom would be lost on exit.
        let now = ui.input(|input| input.time);
        if let Some(wait) = self.state.remember_zoom(ui.ctx().zoom_factor(), now) {
            ui.ctx().request_repaint_after(wait);
        }
        // The window manager says whether the window is maximized; the next launch
        // asks the backend for the same, so a maximized window comes back
        // maximized.
        self.state
            .remember_window(ui.input(|input| input.viewport().maximized));

        // The keyboard goes back to the editor as soon as the terminal is not on
        // screen: a hidden terminal that kept focus would swallow every keystroke of
        // the file being edited. `shown` is set again by the panel when it draws.
        let terminal_was_shown = self.state.terminal.shown;
        self.state.terminal.shown = false;
        if self.state.terminal.focused && !terminal_was_shown {
            self.state.terminal.focused = false;
        }

        // The terminal takes its keys out of the event queue **before** the editor is
        // drawn: otherwise the same keystroke is typed into the open file as well.
        if self.state.terminal.focused {
            let bytes = self.state.terminal.take_keys(ui);
            if !bytes.is_empty() {
                let _ = self.state.terminal.write(&bytes);
            }
        }

        egui::Panel::top("menu_bar").show(ui, |ui| menu_bar(ui, &mut self.state, &mut self.dock));
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            crate::ui::status::bar(ui, &self.state);
        });

        // The theme reaches both the widgets and the text area.
        ui.ctx().set_visuals(match self.state.config.theme {
            ide_core::Theme::Dark => egui::Visuals::dark(),
            ide_core::Theme::Light => egui::Visuals::light(),
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let mut viewer = Viewer {
                state: &mut self.state,
            };
            DockArea::new(&mut self.dock)
                .style(Style::from_egui(ui.style().as_ref()))
                .show_inside(ui, &mut viewer);
        });

        // A link clicked in the terminal: the panel knows the text, the application
        // knows the project and the browser.
        if let Some(link) = self.state.terminal.take_link() {
            match link {
                ide_term::link::Link::Url(url) => {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
                ide_term::link::Link::File { path, line, column } => {
                    self.state.follow_file_link(&path, line, column);
                }
            }
        }

        self.state.apply_pending();
        // The panel is cheap to keep pointed at the right repository: only a change
        // of project does any work here, and only the panel re-reads the status.
        let root = self.state.project.as_ref().map(|project| project.root());
        self.state.git.follow(root);
        if std::mem::take(&mut self.state.git.show_diff) {
            bring_to_front(&mut self.dock, Tab::Diff);
        }
        self.handle_keys(ui);
        self.state.poll_fs_events();
        self.state.poll_lsp();
        // Only documents with unsaved edits need telling, and not on every
        // keystroke: the server reads the file from disk otherwise.
        let now = ui.ctx().input(|input| input.time);
        if now - self.state.last_sync > 0.3 {
            self.state.last_sync = now;
            self.state.sync_open_documents();
        }
        // Quick-open and the prompts are modals: they sit above the dock.
        let picked = {
            let index = self.state.index.as_ref();
            self.state.quick_open.show(ui.ctx(), index)
        };
        if let Some(path) = picked {
            self.state
                .pending
                .push(crate::state::Action::OpenFile(path));
        }
        crate::ui::prompt::show(ui.ctx(), &mut self.state);
        match self.state.rename.show(ui.ctx()) {
            crate::ui::rename::Reply::Submit => self.state.submit_rename(),
            crate::ui::rename::Reply::Apply => self.state.apply_rename(),
            crate::ui::rename::Reply::Cancel | crate::ui::rename::Reply::None => {}
        }
        let chosen = self.state.results.show(ui.ctx());
        if let Some(action) = chosen {
            self.state.pending.push(action);
        }
        // The project-symbol search: its query goes out after the typing has
        // settled, and its answers arrive through `poll_lsp`.
        self.state
            .poll_symbol_search(ui.ctx().input(|input| input.time));
        let picked_symbol = self.state.symbols.show(ui.ctx());
        if let Some(action) = picked_symbol {
            self.state.pending.push(action);
        }
        self.state.poll_navigation();
        self.state.apply_pending();

        // The hint is asked for after the editor drew this frame, so the spot the
        // pointer is over is the one on screen, and painted above everything.
        if let Some(wait) = self.state.track_hover(now) {
            ui.ctx().request_repaint_after(wait);
        }
        if let Some(wait) = self.state.track_highlights(now) {
            ui.ctx().request_repaint_after(wait);
        }
        self.state.hover.show(ui.ctx());

        self.frames += 1;
        self.state.poll_index();
        let mut closing = ui.ctx().input(|input| input.viewport().close_requested());
        if self.state.quit_requested {
            // The user answered the question already: let the window go.
            self.state.quit_requested = false;
            self.state.flush_zoom();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        } else if closing && !self.state.quit_decided && self.state.has_unsaved() {
            // Never lose edits to a stray click on the window's cross. The
            // question is put once, not once a frame: a request that arrives
            // while an answer is pending must not replace what is on screen.
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.state.prompt.is_none() {
                self.state.prompt = Some(crate::state::Prompt::Quit);
            }
            closing = false;
        }
        // The last zoom of the session must not wait for the next launch.
        if closing {
            self.state.flush_zoom();
        }
        // Serializing the dock on every frame costs more than it is worth;
        // layouts change when the user drags a split, not sixty times a second.
        // The dock only has real rectangles after a frame, and a layout with NaN
        // rectangles cannot be read back, so the first frame is skipped.
        let due = closing || now - self.layout_serialized_at >= LAYOUT_SERIALIZE_SECS;
        let layout = if self.frames > 1 && due {
            serde_json::to_string(&self.dock).ok()
        } else {
            None
        };
        if layout.is_some() {
            self.layout_serialized_at = now;
        }
        self.state.save_workspace(layout, now, closing);
    }
}

impl App {
    fn handle_keys(&mut self, ui: &egui::Ui) {
        let close = KeyboardShortcut::new(Modifiers::CTRL, Key::W);
        let save = KeyboardShortcut::new(Modifiers::CTRL, Key::S);
        let save_all = KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::S);
        // Ctrl+Shift+V would paste: the window layer treats any Ctrl+V as a paste,
        // whatever else is held down.
        let preview = KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::M);
        let quick_open = KeyboardShortcut::new(Modifiers::CTRL, Key::P);
        // Alt+F12 is the terminal in IntelliJ, and F12 alone is taken by "go to
        // definition".
        let terminal = KeyboardShortcut::new(Modifiers::ALT, Key::F12);
        // Alt+9 is the Git panel in IntelliJ.
        let git_panel = KeyboardShortcut::new(Modifiers::ALT, Key::Num9);

        // The longer shortcut has to be taken first: Ctrl+Shift+S would
        // otherwise be read as Ctrl+S with a stray modifier.
        let save_all_pressed = ui
            .ctx()
            .input_mut(|input| input.consume_shortcut(&save_all));
        let preview_pressed = ui.ctx().input_mut(|input| input.consume_shortcut(&preview));
        let quick_open_pressed = ui
            .ctx()
            .input_mut(|input| input.consume_shortcut(&quick_open));
        let save_pressed = ui.ctx().input_mut(|input| input.consume_shortcut(&save));
        let close_pressed = ui.ctx().input_mut(|input| input.consume_shortcut(&close));
        let terminal_pressed = ui
            .ctx()
            .input_mut(|input| input.consume_shortcut(&terminal));
        let git_pressed = ui
            .ctx()
            .input_mut(|input| input.consume_shortcut(&git_panel));

        if save_all_pressed {
            self.state.save_all();
        }
        if preview_pressed {
            self.state.preview_open = !self.state.preview_open;
        }
        if terminal_pressed {
            bring_to_front(&mut self.dock, Tab::Terminal);
        }
        if git_pressed {
            bring_to_front(&mut self.dock, Tab::Git);
        }
        if quick_open_pressed {
            self.state.quick_open.open();
        }
        if save_pressed {
            self.state.save_active();
        }
        if close_pressed && let Some(index) = self.state.tabs.active() {
            self.state.close_tab(index);
        }
        // The project symbols keep the key the spec gives them. IntelliJ puts
        // the file structure on Ctrl+F12 and the project search on
        // Ctrl+Alt+Shift+N, and so does this editor.
        let project_symbols = ui.ctx().input_mut(|input| {
            input.consume_key(
                Modifiers::CTRL.plus(Modifiers::SHIFT).plus(Modifiers::ALT),
                Key::N,
            )
        });
        if project_symbols {
            self.state.open_symbol_search();
        }
        let file_symbols = ui
            .ctx()
            .input_mut(|input| input.consume_key(Modifiers::CTRL, Key::F12));
        if file_symbols {
            self.state.file_symbols();
        }
        // Shift+F12 before the bare key: `consume_key` treats a Shift it was not
        // asked about as irrelevant, so checking plain F12 first swallowed the
        // reference list and went to the definition instead.
        let references = ui
            .ctx()
            .input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::F12));
        if references {
            self.state.find_references();
        }
        // Shift+F6 is "rename" in every JetBrains editor, and the modal it opens
        // owns the keyboard while it is up.
        if ui
            .ctx()
            .input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::F6))
        {
            self.state.begin_rename();
        }
        // F12 is the usual "go to definition" everywhere else, and it is a bare
        // key, so it is read from the events rather than as a shortcut.
        let f12 = ui
            .ctx()
            .input_mut(|input| input.consume_key(Modifiers::NONE, Key::F12));
        if f12 {
            self.state.goto_definition();
        }
    }
}

/// Brings a panel's tab to the front, wherever the user has dragged it.
///
/// The keyboard follows to that pane: a panel brought forward is the one that is
/// about to be used.
fn bring_to_front(dock: &mut DockState<Tab>, tab: Tab) {
    if let Some(path) = dock.find_tab(&tab) {
        dock.set_focused_node_and_surface(path.node_path());
        let _ = dock.set_active_tab(path);
    }
}

fn menu_bar(ui: &mut egui::Ui, state: &mut AppState, dock: &mut DockState<Tab>) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("Project", |ui| {
            ui.label("Recent");
            let recent = state.recent.list().to_vec();
            if recent.is_empty() {
                ui.weak("nothing yet");
            }
            for path in recent {
                if ui
                    .button(display_name(&path))
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    state.open_project(&path);
                    ui.close();
                }
            }
        });

        ui.menu_button("View", |ui| {
            if ui
                .checkbox(&mut state.tree.show_hidden, "hidden files")
                .changed()
            {
                state.status = "hidden files toggled".to_owned();
            }
            if ui
                .checkbox(&mut state.preview_open, "markdown preview")
                .changed()
            {
                state.status = "markdown preview toggled".to_owned();
            }
            if ui.button("quick open	Ctrl+P").clicked() {
                state.quick_open.open();
                ui.close();
            }
            ui.separator();
            // The bottom of the window is a row of tabs, so a panel is "opened" by
            // bringing its tab to the front.
            if ui.button("problems	Ctrl+Shift+M").clicked() {
                bring_to_front(dock, Tab::Problems);
                ui.close();
            }
            if ui.button("terminal	Alt+F12").clicked() {
                bring_to_front(dock, Tab::Terminal);
                ui.close();
            }
            if ui.button("git	Alt+9").clicked() {
                bring_to_front(dock, Tab::Git);
                ui.close();
            }
            ui.separator();
            let mut dark = state.config.theme == ide_core::Theme::Dark;
            if ui.checkbox(&mut dark, "dark theme").changed() {
                state.config.theme = if dark {
                    ide_core::Theme::Dark
                } else {
                    ide_core::Theme::Light
                };
                state.save_config();
            }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match &state.project {
                Some(project) => ui.label(display_name(project.root())),
                None => ui.weak("no project"),
            };
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_project(dir: &std::path::Path, config: &std::path::Path) -> App {
        App {
            state: AppState::with_config_dir(config.to_path_buf(), Some(dir.to_path_buf()), None),
            dock: default_layout(),
            zoom_applied: false,
            frames: 0,
            layout_serialized_at: 0.0,
        }
    }

    fn draw_frames(app: &mut App, frames: usize) -> usize {
        let ctx = egui::Context::default();
        let mut shapes = 0;
        for _ in 0..frames {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.draw(ui));
            shapes = output.shapes.len();
            output.textures_delta.clear();
        }
        shapes
    }

    #[test]
    fn default_layout_holds_every_panel() {
        let dock = default_layout();
        let tabs: Vec<Tab> = dock.iter_all_tabs().map(|(_, tab)| *tab).collect();
        assert_eq!(tabs.len(), 6, "every panel is part of the layout: {tabs:?}");
        assert!(tabs.contains(&Tab::Projects));
        assert!(tabs.contains(&Tab::Editor));
        assert!(tabs.contains(&Tab::Problems));
        assert!(tabs.contains(&Tab::Terminal));
        assert!(tabs.contains(&Tab::Git));
        assert!(tabs.contains(&Tab::Diff));
    }

    #[test]
    fn draws_frames_with_a_file_open() {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut app = app_with_project(project.path(), config.path());
        app.state.open_file(std::path::Path::new("main.rs"));

        let shapes = draw_frames(&mut app, 2);
        assert!(shapes > 0);
        assert_eq!(app.state.tabs.len(), 1);
    }

    #[test]
    fn draws_without_a_project() {
        let config = tempfile::tempdir().unwrap();
        let mut app = App {
            state: AppState::with_config_dir(config.path().to_path_buf(), None, None),
            dock: default_layout(),
            zoom_applied: false,
            frames: 0,
            layout_serialized_at: 0.0,
        };
        let shapes = draw_frames(&mut app, 1);
        assert!(shapes > 0);
    }
}

#[cfg(test)]
mod typing_tests {
    use super::*;

    fn frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) -> usize {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(900.0, 700.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.draw(ui));
        let shapes = output.shapes.len();
        output.textures_delta.clear();
        shapes
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn setup() -> (tempfile::TempDir, tempfile::TempDir, App) {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.rs"), "fn main() {}\n").unwrap();
        let mut app = App {
            state: AppState::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            ),
            dock: default_layout(),
            zoom_applied: false,
            frames: 0,
            layout_serialized_at: 0.0,
        };
        app.state.open_file(std::path::Path::new("a.rs"));
        (project, config, app)
    }

    #[test]
    fn typed_text_reaches_the_buffer_and_dirties_the_tab() {
        let (_project, _config, mut app) = setup();
        let ctx = egui::Context::default();

        // First frame lays the widget out and takes focus.
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, vec![egui::Event::Text("X".to_owned())]);

        let buffer = app.state.active_buffer().expect("active buffer");
        assert!(buffer.text().starts_with('X'), "text: {:?}", buffer.text());
        assert!(buffer.is_dirty());
        assert!(app.state.tabs.dirty(0));
    }

    #[test]
    fn ctrl_s_writes_the_file() {
        let (project, _config, mut app) = setup();
        let ctx = egui::Context::default();

        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, vec![egui::Event::Text("Y".to_owned())]);
        frame(
            &ctx,
            &mut app,
            vec![key_event(egui::Key::S, egui::Modifiers::CTRL)],
        );

        let on_disk = std::fs::read_to_string(project.path().join("a.rs")).unwrap();
        assert_eq!(on_disk, "Yfn main() {}\n");
        assert!(!app.state.active_buffer().unwrap().is_dirty());
    }

    #[test]
    fn backspace_and_arrows_move_the_caret() {
        let (_project, _config, mut app) = setup();
        let ctx = egui::Context::default();

        frame(&ctx, &mut app, Vec::new());
        frame(
            &ctx,
            &mut app,
            vec![key_event(egui::Key::End, egui::Modifiers::NONE)],
        );
        frame(
            &ctx,
            &mut app,
            vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        frame(
            &ctx,
            &mut app,
            vec![key_event(egui::Key::Backspace, egui::Modifiers::NONE)],
        );

        let buffer = app.state.active_buffer().unwrap();
        assert_eq!(buffer.line_count(), 2);
        assert_eq!(buffer.caret(), 12);
    }
}
#[cfg(test)]
#[cfg(test)]
mod layout_tests {
    use super::*;

    /// Painted line numbers, i.e. the rows the editor laid out this frame.
    fn painted_numbers(app: &mut App) -> Vec<usize> {
        let ctx = egui::Context::default();
        let mut output = None;
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            };
            let mut frame = ctx.run_ui(input, |ui| app.draw(ui));
            frame.textures_delta.clear();
            output = Some(frame);
        }
        let mut output = output.expect("frames drawn");
        let numbers = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => text.galley.text().trim().parse::<usize>().ok(),
                _ => None,
            })
            .collect::<Vec<_>>();
        output.textures_delta.clear();
        numbers
    }

    fn app_with_long_file() -> (tempfile::TempDir, tempfile::TempDir, App) {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let text: String = (1..=400)
            .map(|n| format!("let value_{n} = {n};\n"))
            .collect();
        std::fs::write(project.path().join("long.rs"), text).unwrap();
        let mut app = App {
            state: AppState::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            ),
            dock: default_layout(),
            zoom_applied: false,
            frames: 0,
            layout_serialized_at: 0.0,
        };
        app.state.open_file(std::path::Path::new("long.rs"));
        (project, config, app)
    }

    /// The editor pane must take most of the window: a split fraction that is
    /// applied the wrong way round leaves the editor a narrow strip.
    #[test]
    fn editor_pane_gets_the_bulk_of_the_window() {
        let (_project, _config, mut app) = app_with_long_file();
        let numbers = painted_numbers(&mut app);
        assert!(
            numbers.len() >= 20,
            "only {} line numbers were laid out; the editor pane is too small",
            numbers.len()
        );
        assert_eq!(*numbers.iter().min().unwrap(), 1);
    }
}

#[cfg(test)]
mod click_tests {
    use super::*;

    fn run(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>, time: f64) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(900.0, 700.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.draw(ui));
        output.textures_delta.clear();
    }

    fn press(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn release(at: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    /// A click inside the text must move the caret there.
    #[test]
    fn clicking_in_the_text_moves_the_caret() {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let text: String = (0..40).map(|n| format!("line number {n}\n")).collect();
        std::fs::write(project.path().join("a.rs"), text).unwrap();
        let mut app = App {
            state: AppState::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            ),
            dock: default_layout(),
            zoom_applied: false,
            frames: 0,
            layout_serialized_at: 0.0,
        };
        app.state.open_file(std::path::Path::new("a.rs"));
        let ctx = egui::Context::default();

        let empty_project = tempfile::tempdir().unwrap();
        std::fs::write(empty_project.path().join("x.rs"), "").unwrap();

        run(&ctx, &mut app, Vec::new(), 0.1);
        run(&ctx, &mut app, Vec::new(), 0.2);
        assert_eq!(app.state.active_buffer().unwrap().caret(), 0);

        // Somewhere in the middle of the visible text.
        let at = egui::pos2(320.0, 200.0);
        run(&ctx, &mut app, press(at), 0.3);
        run(&ctx, &mut app, release(at), 0.4);

        let buffer = app.state.active_buffer().unwrap();
        let (line, col) = buffer.line_col();
        assert!(
            (2..12).contains(&line),
            "click placed the caret on line {line}, column {col}"
        );
    }
}

#[cfg(test)]
mod layout_persistence_tests {
    use super::*;
    use ide_core::{Workspace, WorkspaceState};

    fn drawn_app() -> (tempfile::TempDir, tempfile::TempDir, App) {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.rs"), "fn main() {}\n").unwrap();
        let mut app = App {
            state: AppState::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            ),
            dock: default_layout(),
            zoom_applied: false,
            frames: 0,
            layout_serialized_at: 0.0,
        };
        app.state.open_file(std::path::Path::new("a.rs"));
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.draw(ui));
            output.textures_delta.clear();
        }
        (project, config, app)
    }

    #[test]
    fn a_drawn_layout_round_trips_through_json() {
        let (_project, _config, app) = drawn_app();
        let json = serde_json::to_string(&app.dock).expect("layout serializes");
        let back: DockState<Tab> = serde_json::from_str(&json).expect("layout reads back");
        assert_eq!(back.iter_all_tabs().count(), 6);
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn a_stored_layout_is_restored_at_startup() {
        let (project, config, app) = drawn_app();
        let layout = serde_json::to_string(&app.dock).unwrap();

        let workspace = Workspace::open(&project.path().canonicalize().unwrap()).unwrap();
        workspace
            .write_state(&WorkspaceState {
                tabs: vec![],
                active: None,
                layout: Some(layout.clone()),
            })
            .unwrap();

        let restored = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        assert_eq!(restored.dock.iter_all_tabs().count(), 6);
        assert_eq!(serde_json::to_string(&restored.dock).unwrap(), layout);
    }

    #[test]
    fn a_broken_layout_falls_back_to_the_default() {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(&project.path().canonicalize().unwrap()).unwrap();
        workspace
            .write_state(&WorkspaceState {
                tabs: vec![],
                active: None,
                layout: Some("{\"nonsense\": true}".to_owned()),
            })
            .unwrap();

        let app = App::new(Some(project.path().to_path_buf()), None);
        let _ = config;
        assert_eq!(app.dock.iter_all_tabs().count(), 6);
    }
}

#[cfg(test)]
mod autosave_tests {
    use super::*;

    #[test]
    fn drawing_frames_saves_the_workspace() {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.rs"), "fn main() {}\n").unwrap();
        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("a.rs"));

        let ctx = egui::Context::default();
        let mut time = 0.0;
        for _ in 0..4 {
            time += 0.4;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(time),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.draw(ui));
            output.textures_delta.clear();
        }

        let text = std::fs::read_to_string(
            project
                .path()
                .canonicalize()
                .unwrap()
                .join(".andy-ide/workspace.json"),
        )
        .expect("workspace.json written");

        assert!(text.contains("a.rs"), "tabs are saved: {text}");
        assert!(text.contains("layout"), "layout is saved: {text}");
        assert!(!text.contains("null"), "the layout has no NaN rectangles");
    }
}

#[cfg(test)]
mod keyboard_tests {
    use super::*;

    struct Harness {
        ctx: egui::Context,
        app: App,
        time: f64,
        _project: tempfile::TempDir,
        _config: tempfile::TempDir,
    }

    impl Harness {
        fn new(lines: usize) -> Self {
            let project = tempfile::tempdir().unwrap();
            let config = tempfile::tempdir().unwrap();
            let text: String = (0..lines).map(|n| format!("line number {n}\n")).collect();
            std::fs::write(project.path().join("a.rs"), text).unwrap();
            let mut app = App::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            );
            app.state.open_file(std::path::Path::new("a.rs"));
            let mut harness = Harness {
                ctx: egui::Context::default(),
                app,
                time: 0.0,
                _project: project,
                _config: config,
            };
            harness.frame(Vec::new());
            harness.frame(Vec::new());
            harness
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.time += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| self.app.draw(ui));
            output.textures_delta.clear();
        }

        fn click_in_text(&mut self) {
            let at = egui::pos2(320.0, 150.0);
            self.frame(vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
        }

        fn key(&mut self, key: egui::Key) {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
        }

        fn caret_line(&self) -> usize {
            self.app.state.active_buffer().unwrap().line_col().0
        }

        fn focused(&self) -> Option<egui::Id> {
            self.ctx.memory(|memory| memory.focused())
        }
    }

    /// The arrow keys must move the caret, not the keyboard focus between panels.
    #[test]
    fn arrow_up_moves_the_caret_and_keeps_the_focus() {
        let mut harness = Harness::new(30);
        harness.click_in_text();
        let focus_before = harness.focused();
        assert!(focus_before.is_some(), "the editor never took focus");

        harness.key(egui::Key::ArrowDown);
        let after_down = harness.caret_line();
        assert!(after_down > 0, "arrow down did not move the caret");

        harness.key(egui::Key::ArrowUp);
        assert_eq!(
            harness.caret_line(),
            after_down - 1,
            "arrow up moved to line {} instead of {}",
            harness.caret_line(),
            after_down - 1
        );
        assert_eq!(
            harness.focused(),
            focus_before,
            "arrow keys moved the focus"
        );
    }

    #[test]
    fn arrow_left_and_right_stay_inside_the_editor() {
        let mut harness = Harness::new(10);
        harness.click_in_text();
        let focus_before = harness.focused();

        let (line, col) = harness.app.state.active_buffer().unwrap().line_col();

        harness.key(egui::Key::ArrowRight);
        harness.key(egui::Key::ArrowRight);
        harness.key(egui::Key::ArrowLeft);

        assert_eq!(
            harness.focused(),
            focus_before,
            "arrow keys moved the focus"
        );
        let (after_line, after_col) = harness.app.state.active_buffer().unwrap().line_col();
        assert_eq!(after_line, line);
        assert_eq!(after_col, col + 1);
    }
}

#[cfg(test)]
mod caret_and_clipboard_tests {
    use super::*;

    struct Harness {
        ctx: egui::Context,
        app: App,
        time: f64,
        _project: tempfile::TempDir,
        _config: tempfile::TempDir,
    }

    impl Harness {
        fn with_text(name: &str, text: &str) -> Self {
            let project = tempfile::tempdir().unwrap();
            let config = tempfile::tempdir().unwrap();
            std::fs::write(project.path().join(name), text).unwrap();
            let mut app = App::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            );
            app.state.open_file(std::path::Path::new(name));
            let mut harness = Harness {
                ctx: egui::Context::default(),
                app,
                time: 0.0,
                _project: project,
                _config: config,
            };
            harness.frame(Vec::new());
            harness.frame(Vec::new());
            harness
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.time += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| self.app.draw(ui));
            output.textures_delta.clear();
            output
        }

        fn focus_editor(&mut self) {
            let at = egui::pos2(320.0, 150.0);
            self.frame(vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
        }

        fn key_with(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }]);
        }

        fn key(&mut self, key: egui::Key) {
            self.key_with(key, egui::Modifiers::NONE);
        }

        fn buffer(&self) -> &crate::editor::EditorBuffer {
            self.app.state.active_buffer().unwrap()
        }

        fn text(&self) -> String {
            self.buffer().text().to_owned()
        }

        fn copied(&self, output: &egui::FullOutput) -> Option<String> {
            output.platform_output.commands.iter().find_map(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    Some(text.clone())
                } else {
                    None
                }
            })
        }
    }

    /// End must land exactly on the last character of the line, whatever the
    /// line holds: tabs, Cyrillic, a carriage return or trailing spaces.
    #[test]
    fn end_lands_on_the_true_end_of_every_line() {
        let text = "let x = 1;\n\tindented\nпривет мир\nwindows\r  \nlast";
        let mut harness = Harness::with_text("a.rs", text);
        harness.focus_editor();

        let lines: Vec<String> = {
            let buffer = harness.buffer();
            (0..buffer.line_count())
                .map(|line| buffer.line_text(line).to_owned())
                .collect()
        };

        for (line, text) in lines.iter().enumerate() {
            harness.key_with(egui::Key::Home, egui::Modifiers::CTRL);
            for _ in 0..line {
                harness.key(egui::Key::ArrowDown);
            }
            harness.key_with(egui::Key::Home, egui::Modifiers::NONE);
            harness.key(egui::Key::End);

            let (caret_line, caret_col) = harness.buffer().line_col();
            assert_eq!(caret_line, line, "caret left line {line}");
            assert_eq!(
                caret_col,
                text.chars().count(),
                "End on line {line} ({text:?}) went to column {caret_col}"
            );

            // A second End must not move anywhere.
            harness.key(egui::Key::End);
            assert_eq!(harness.buffer().line_col(), (line, caret_col));
        }
    }

    #[test]
    fn copy_puts_the_selection_on_the_clipboard() {
        let mut harness = Harness::with_text("a.rs", "let x = 1;\n");
        harness.focus_editor();
        harness.key_with(egui::Key::A, egui::Modifiers::CTRL);

        let output = harness.frame(vec![egui::Event::Copy]);
        assert_eq!(harness.copied(&output).as_deref(), Some("let x = 1;\n"));
        // Copying must not change the text.
        assert_eq!(harness.text(), "let x = 1;\n");
    }

    #[test]
    fn cut_removes_the_selection_and_copies_it() {
        let mut harness = Harness::with_text("a.rs", "keep cut\n");
        harness.focus_editor();
        harness.key_with(egui::Key::Home, egui::Modifiers::CTRL);
        for _ in 0..4 {
            harness.key_with(egui::Key::ArrowRight, egui::Modifiers::SHIFT);
        }
        assert_eq!(harness.buffer().selected_text(), "keep");

        let output = harness.frame(vec![egui::Event::Cut]);
        assert_eq!(harness.copied(&output).as_deref(), Some("keep"));
        assert_eq!(harness.text(), " cut\n");
    }

    #[test]
    fn paste_inserts_at_the_caret() {
        let mut harness = Harness::with_text("a.rs", "ab\n");
        harness.focus_editor();
        harness.key_with(egui::Key::Home, egui::Modifiers::CTRL);
        harness.key(egui::Key::ArrowRight);

        harness.frame(vec![egui::Event::Paste("XY".to_owned())]);
        assert_eq!(harness.text(), "aXYb\n");
        assert_eq!(harness.buffer().line_col(), (0, 3));
    }

    #[test]
    fn tab_indents_the_selected_lines_from_the_keyboard() {
        let mut harness = Harness::with_text("a.rs", "one\ntwo\n");
        harness.focus_editor();
        harness.key_with(egui::Key::Home, egui::Modifiers::CTRL);
        harness.key(egui::Key::End);
        harness.key_with(egui::Key::ArrowDown, egui::Modifiers::SHIFT);
        // From the end of the first line to the end of the second: the whole
        // lines are indented even though the selection starts mid-line.
        assert_eq!(harness.buffer().selected_text(), "\ntwo");

        harness.key(egui::Key::Tab);
        assert_eq!(harness.text(), "    one\n    two\n");
        assert_eq!(harness.buffer().selected_text(), "    one\n    two");

        harness.key_with(egui::Key::Tab, egui::Modifiers::SHIFT);
        assert_eq!(harness.text(), "one\ntwo\n");
    }

    #[test]
    fn paste_replaces_the_selection() {
        let mut harness = Harness::with_text("a.rs", "ab\n");
        harness.focus_editor();
        harness.key_with(egui::Key::A, egui::Modifiers::CTRL);

        harness.frame(vec![egui::Event::Paste("Z".to_owned())]);
        assert_eq!(harness.text(), "Z");
    }
}

#[cfg(test)]
mod drag_scroll_tests {
    use super::*;

    struct Harness {
        ctx: egui::Context,
        app: App,
        time: f64,
        _project: tempfile::TempDir,
        _config: tempfile::TempDir,
    }

    impl Harness {
        fn with_lines(lines: usize) -> Self {
            let project = tempfile::tempdir().unwrap();
            let config = tempfile::tempdir().unwrap();
            let text: String = (0..lines).map(|n| format!("line number {n}\n")).collect();
            std::fs::write(project.path().join("a.rs"), text).unwrap();
            let mut app = App::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            );
            app.state.open_file(std::path::Path::new("a.rs"));
            let mut harness = Harness {
                ctx: egui::Context::default(),
                app,
                time: 0.0,
                _project: project,
                _config: config,
            };
            harness.frame(Vec::new());
            harness.frame(Vec::new());
            harness
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.time += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| self.app.draw(ui));
            output.textures_delta.clear();
        }

        fn press(&mut self, at: egui::Pos2) {
            self.frame(vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }

        fn drag_to(&mut self, at: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(at)]);
        }

        fn selection_end_line(&self) -> usize {
            let buffer = self.app.state.active_buffer().unwrap();
            buffer.doc.char_to_line_col(buffer.selection().end).0
        }
    }

    /// A selection that runs past the bottom edge has to scroll, and has to keep
    /// scrolling while the button is held, so the far edge stays visible.
    #[test]
    fn dragging_past_the_bottom_keeps_scrolling() {
        let mut harness = Harness::with_lines(200);
        harness.press(egui::pos2(320.0, 150.0));
        harness.drag_to(egui::pos2(320.0, 300.0));
        harness.drag_to(egui::pos2(320.0, 690.0));

        let mut ends = Vec::new();
        for _ in 0..6 {
            harness.drag_to(egui::pos2(320.0, 690.0));
            ends.push(harness.selection_end_line());
        }

        assert!(
            ends[0] > 10,
            "the selection stopped at line {} while the pointer was at the bottom edge",
            ends[0]
        );
        // One row per frame: a jump would mean the view is being dragged along
        // by the caret instead of following the pointer.
        let steps: Vec<i64> = ends
            .windows(2)
            .map(|pair| pair[1] as i64 - pair[0] as i64)
            .collect();
        assert!(
            steps.iter().all(|step| (1..=2).contains(step)),
            "the view scrolled unevenly: {steps:?} from {ends:?}"
        );
    }
}

#[cfg(test)]
mod caret_visibility_tests {
    use super::*;

    struct Harness {
        ctx: egui::Context,
        app: App,
        time: f64,
        _project: tempfile::TempDir,
        _config: tempfile::TempDir,
    }

    impl Harness {
        fn with_lines(lines: usize) -> Self {
            let project = tempfile::tempdir().unwrap();
            let config = tempfile::tempdir().unwrap();
            let text: String = (0..lines).map(|n| format!("line number {n}\n")).collect();
            std::fs::write(project.path().join("a.rs"), text).unwrap();
            let mut app = App::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            );
            app.state.open_file(std::path::Path::new("a.rs"));
            let mut harness = Harness {
                ctx: egui::Context::default(),
                app,
                time: 0.0,
                _project: project,
                _config: config,
            };
            harness.frame(Vec::new());
            harness.frame(Vec::new());
            harness
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.time += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| self.app.draw(ui));
            output.textures_delta.clear();
            output
        }

        fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> egui::FullOutput {
            self.key_rows(key, modifiers).1
        }

        /// Presses a key and returns the caret rows of the last frame that painted
        /// a caret, with the frame's output.
        ///
        /// The caret blinks, so the last frame drawn may well be a dark one: a test
        /// that looks only at it would report a caret that is not there.
        fn key_rows(
            &mut self,
            key: egui::Key,
            modifiers: egui::Modifiers,
        ) -> (Vec<f32>, egui::FullOutput) {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }]);
            // Smooth scrolling needs a few frames to arrive; the answer is what
            // the last of them painted.
            let mut lit = None;
            let mut output = None;
            for _ in 0..8 {
                let frame = self.frame(Vec::new());
                let rows = Self::caret_rows(&frame);
                if !rows.is_empty() {
                    lit = Some(rows);
                }
                output = Some(frame);
            }
            (lit.unwrap_or_default(), output.expect("frames drawn"))
        }

        /// Y of the carets that were painted this frame, in screen points.
        fn caret_rows(output: &egui::FullOutput) -> Vec<f32> {
            let mut rows = Vec::new();
            for clipped in &output.shapes {
                if let egui::Shape::Rect(rect) = &clipped.shape
                    && (rect.rect.width() - 1.5).abs() < 0.01
                {
                    rows.push(rect.rect.min.y);
                }
            }
            rows
        }
    }

    /// A caret that is always painted is easy to lose in the text: the bar sits
    /// between two glyphs and reads as one of them. It blinks in the real draw
    /// path, not only in the view's own test.
    #[test]
    fn the_caret_blinks_in_the_drawn_output() {
        let mut harness = Harness::with_lines(20);
        harness.key(egui::Key::End, egui::Modifiers::CTRL);

        // 1.2 s at 0.1 s a frame: two turns of a 0.53 s blink, so the caret has to
        // be seen and lost more than once.
        let mut lit = 0;
        let mut dark = 0;
        for _ in 0..12 {
            if Harness::caret_rows(&harness.frame(Vec::new())).is_empty() {
                dark += 1;
            } else {
                lit += 1;
            }
        }
        assert!(
            lit >= 3 && dark >= 3,
            "the caret did not blink in the drawn output: lit {lit}, dark {dark} frames"
        );
    }

    #[test]
    fn ctrl_home_leaves_the_caret_where_it_can_be_seen() {
        let mut harness = Harness::with_lines(200);
        harness.key(egui::Key::End, egui::Modifiers::CTRL);
        let (rows, _output) = harness.key_rows(egui::Key::End, egui::Modifiers::CTRL);
        assert!(!rows.is_empty(), "no caret was painted");
        let visible = rows.iter().any(|y| (0.0..700.0).contains(y));
        assert!(visible, "the caret is off screen after Ctrl+End: {rows:?}");

        let (rows, _output) = harness.key_rows(egui::Key::Home, egui::Modifiers::CTRL);
        assert!(
            rows.iter().any(|y| (0.0..700.0).contains(y)),
            "the caret is off screen after Ctrl+Home: {rows:?}"
        );
        assert_eq!(harness.app.state.active_buffer().unwrap().caret(), 0);
    }
}

#[cfg(test)]
mod clipboard_keys_tests {
    use super::*;

    struct Harness {
        ctx: egui::Context,
        app: App,
        time: f64,
        _project: tempfile::TempDir,
        _config: tempfile::TempDir,
    }

    impl Harness {
        fn with_text(name: &str, text: &str) -> Self {
            let project = tempfile::tempdir().unwrap();
            let config = tempfile::tempdir().unwrap();
            std::fs::write(project.path().join(name), text).unwrap();
            let mut app = App::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            );
            app.state.open_file(std::path::Path::new(name));
            let mut harness = Harness {
                ctx: egui::Context::default(),
                app,
                time: 0.0,
                _project: project,
                _config: config,
            };
            harness.frame(Vec::new());
            harness.frame(Vec::new());
            harness
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.time += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| self.app.draw(ui));
            output.textures_delta.clear();
            output
        }

        fn focus_editor(&mut self) {
            let at = egui::pos2(320.0, 150.0);
            self.frame(vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
        }

        fn key_with(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> egui::FullOutput {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }])
        }

        fn select_first_line(&mut self) {
            self.key_with(egui::Key::Home, egui::Modifiers::CTRL);
        }

        fn text(&self) -> String {
            self.app.state.active_buffer().unwrap().text().to_owned()
        }

        fn selection(&self) -> String {
            self.app.state.active_buffer().unwrap().selected_text()
        }

        fn copied(&self, output: &egui::FullOutput) -> Option<String> {
            output.platform_output.commands.iter().find_map(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    Some(text.clone())
                } else {
                    None
                }
            })
        }

        fn asked_for_a_paste(&self, output: &egui::FullOutput) -> bool {
            output
                .viewport_output
                .values()
                .flat_map(|viewport| viewport.commands.iter())
                .any(|command| matches!(command, egui::ViewportCommand::RequestPaste))
        }
    }

    #[test]
    fn shift_delete_cuts_the_selection() {
        let mut harness = Harness::with_text("a.rs", "keep and cut\n");
        harness.focus_editor();
        harness.select_first_line();
        for _ in 0..4 {
            harness.key_with(egui::Key::ArrowRight, egui::Modifiers::SHIFT);
        }
        assert_eq!(harness.selection(), "keep");

        let output = harness.key_with(egui::Key::Delete, egui::Modifiers::SHIFT);
        assert_eq!(harness.copied(&output).as_deref(), Some("keep"));
        assert_eq!(harness.text(), " and cut\n");
    }

    #[test]
    fn ctrl_insert_copies_the_selection() {
        let mut harness = Harness::with_text("a.rs", "keep and cut\n");
        harness.focus_editor();
        harness.select_first_line();
        for _ in 0..4 {
            harness.key_with(egui::Key::ArrowRight, egui::Modifiers::SHIFT);
        }

        let output = harness.key_with(egui::Key::Insert, egui::Modifiers::CTRL);
        assert_eq!(harness.copied(&output).as_deref(), Some("keep"));
        assert_eq!(harness.text(), "keep and cut\n", "copying changes nothing");
    }

    #[test]
    fn shift_insert_asks_the_backend_for_the_clipboard() {
        let mut harness = Harness::with_text("a.rs", "ab\n");
        harness.focus_editor();

        let output = harness.key_with(egui::Key::Insert, egui::Modifiers::SHIFT);
        assert!(
            harness.asked_for_a_paste(&output),
            "the editor must ask the window for the clipboard text"
        );
        assert_eq!(harness.text(), "ab\n", "asking changes nothing by itself");
    }

    #[test]
    fn plain_delete_still_deletes_forward() {
        let mut harness = Harness::with_text("a.rs", "ab\n");
        harness.focus_editor();
        harness.select_first_line();

        harness.key_with(egui::Key::Delete, egui::Modifiers::NONE);
        assert_eq!(harness.text(), "b\n");
    }

    #[test]
    fn copying_without_a_selection_takes_the_whole_line() {
        let mut harness = Harness::with_text("a.rs", "first\nsecond\n");
        harness.focus_editor();
        harness.select_first_line();
        harness.key_with(egui::Key::ArrowDown, egui::Modifiers::NONE);

        let output = harness.key_with(egui::Key::Insert, egui::Modifiers::CTRL);
        assert_eq!(harness.copied(&output).as_deref(), Some("second\n"));
        assert_eq!(harness.text(), "first\nsecond\n");
    }

    #[test]
    fn cutting_without_a_selection_removes_the_line() {
        let mut harness = Harness::with_text("a.rs", "first\nsecond\nthird\n");
        harness.focus_editor();
        harness.select_first_line();
        harness.key_with(egui::Key::ArrowDown, egui::Modifiers::NONE);

        let output = harness.key_with(egui::Key::Delete, egui::Modifiers::SHIFT);
        assert_eq!(harness.copied(&output).as_deref(), Some("second\n"));
        assert_eq!(harness.text(), "first\nthird\n");
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use ide_core::FsEvent;
    use std::path::PathBuf;

    struct Harness {
        ctx: egui::Context,
        app: App,
        time: f64,
        root: PathBuf,
        _project: tempfile::TempDir,
        _config: tempfile::TempDir,
    }

    impl Harness {
        fn new(files: &[(&str, &str)]) -> Self {
            let project = tempfile::tempdir().unwrap();
            let config = tempfile::tempdir().unwrap();
            for (name, text) in files {
                std::fs::write(project.path().join(name), text).unwrap();
            }
            let mut app = App::with_config_dir(
                config.path().to_path_buf(),
                Some(project.path().to_path_buf()),
                None,
            );
            // The watcher would deliver the very events the tests hand over by
            // hand, and the two would race.
            app.state.pause_watching();
            let root = project.path().to_path_buf();
            for (name, _) in files {
                app.state.open_file(std::path::Path::new(name));
            }
            let mut harness = Harness {
                ctx: egui::Context::default(),
                app,
                time: 0.0,
                root,
                _project: project,
                _config: config,
            };
            harness.frame(Vec::new());
            harness.frame(Vec::new());
            harness
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.frame_with(events, Vec::new())
        }

        fn frame_with(
            &mut self,
            events: Vec<egui::Event>,
            viewport_events: Vec<egui::ViewportEvent>,
        ) -> egui::FullOutput {
            self.time += 0.1;
            let mut viewports = egui::ViewportIdMap::default();
            viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    events: viewport_events,
                    ..Default::default()
                },
            );
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(self.time),
                events,
                viewports,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| self.app.draw(ui));
            output.textures_delta.clear();
            output
        }

        fn text_of(&self, name: &str) -> String {
            self.app
                .state
                .buffers
                .values()
                .find(|buffer| buffer.path == std::path::Path::new(name))
                .map(|buffer| buffer.text().to_owned())
                .unwrap_or_default()
        }

        fn type_into(&mut self, name: &str, text: &str) {
            let id = self
                .app
                .state
                .tabs
                .items()
                .iter()
                .find(|item| item.path == std::path::Path::new(name))
                .map(|item| item.buffer_id)
                .expect("the file is open");
            let buffer = self.app.state.buffers.get_mut(&id).unwrap();
            buffer.insert_text(text);
            self.app.state.sync_dirty();
        }

        fn write_on_disk(&self, name: &str, text: &str) {
            std::fs::write(self.root.join(name), text).unwrap();
        }

        fn change_on_disk(&mut self, name: &str, text: &str) {
            self.write_on_disk(name, text);
            self.app
                .state
                .handle_fs_event(FsEvent::Changed(self.root.join(name)));
        }

        fn commands(&self, output: &egui::FullOutput) -> Vec<egui::ViewportCommand> {
            output
                .viewport_output
                .values()
                .flat_map(|viewport| viewport.commands.clone())
                .collect()
        }
    }

    #[test]
    fn a_change_on_disk_reloads_a_clean_buffer() {
        let mut harness = Harness::new(&[("a.rs", "one\n")]);
        harness.change_on_disk("a.rs", "two\n");

        assert_eq!(harness.text_of("a.rs"), "two\n");
        let buffer = harness
            .app
            .state
            .buffers
            .values()
            .find(|buffer| buffer.path == std::path::Path::new("a.rs"))
            .unwrap();
        assert!(!buffer.is_dirty(), "the text is the text on disk");
        assert!(
            harness.app.state.status.contains("reloaded"),
            "status was {:?}",
            harness.app.state.status
        );
    }

    #[test]
    fn a_change_on_disk_asks_when_the_buffer_has_edits() {
        let mut harness = Harness::new(&[("a.rs", "one\n")]);
        harness.type_into("a.rs", "mine ");

        harness.change_on_disk("a.rs", "theirs\n");

        assert_eq!(
            harness.text_of("a.rs"),
            "mine one\n",
            "the edits are kept until the question is answered"
        );
        assert_eq!(
            harness.app.state.prompt,
            Some(crate::state::Prompt::ExternalChange {
                path: PathBuf::from("a.rs")
            })
        );

        // Reload throws the edits away and takes the disk text.
        harness
            .app
            .state
            .pending
            .push(crate::state::Action::ReloadExternal(PathBuf::from("a.rs")));
        harness.frame(Vec::new());
        assert_eq!(harness.text_of("a.rs"), "theirs\n");
        assert!(harness.app.state.prompt.is_none());
    }

    #[test]
    fn keeping_mine_leaves_the_buffer_alone() {
        let mut harness = Harness::new(&[("a.rs", "one\n")]);
        harness.type_into("a.rs", "mine ");
        harness.change_on_disk("a.rs", "theirs\n");

        harness
            .app
            .state
            .pending
            .push(crate::state::Action::KeepMine(PathBuf::from("a.rs")));
        harness.frame(Vec::new());

        assert_eq!(harness.text_of("a.rs"), "mine one\n");
        assert!(harness.app.state.prompt.is_none());
        assert!(
            harness
                .app
                .state
                .buffers
                .values()
                .find(|buffer| buffer.path == std::path::Path::new("a.rs"))
                .unwrap()
                .is_dirty()
        );
    }

    #[test]
    fn the_editors_own_save_is_not_a_change() {
        let mut harness = Harness::new(&[("a.rs", "one\n")]);
        harness.app.state.save_active();
        assert!(!harness.app.state.status.contains("reloaded"));

        // The watcher reports the very write the editor just did.
        harness
            .app
            .state
            .handle_fs_event(FsEvent::Changed(harness.root.join("a.rs")));
        assert!(harness.app.state.prompt.is_none(), "nothing to ask about");
        assert_eq!(harness.text_of("a.rs"), "one\n");
    }

    #[test]
    fn a_file_removed_on_disk_is_reported() {
        let mut harness = Harness::new(&[("a.rs", "one\n")]);
        std::fs::remove_file(harness.root.join("a.rs")).unwrap();
        harness
            .app
            .state
            .handle_fs_event(FsEvent::Removed(harness.root.join("a.rs")));

        assert!(harness.app.state.status.contains("removed"));
        assert_eq!(harness.text_of("a.rs"), "one\n", "the buffer is not lost");
    }

    #[test]
    fn save_all_writes_every_dirty_tab() {
        let mut harness = Harness::new(&[("a.rs", "a\n"), ("b.rs", "b\n")]);
        harness.type_into("a.rs", "1");
        harness.type_into("b.rs", "2");

        let output = harness.frame(vec![egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        }]);
        let _ = output;

        assert_eq!(
            std::fs::read_to_string(harness.root.join("a.rs")).unwrap(),
            "1a\n"
        );
        assert_eq!(
            std::fs::read_to_string(harness.root.join("b.rs")).unwrap(),
            "2b\n"
        );
        assert!(!harness.app.state.has_unsaved());
    }

    #[test]
    fn closing_a_dirty_tab_asks_before_dropping_it() {
        let mut harness = Harness::new(&[("a.rs", "one\n")]);
        harness.type_into("a.rs", "x");

        harness.app.state.close_tab(0);
        assert_eq!(
            harness.app.state.prompt,
            Some(crate::state::Prompt::CloseTab { index: 0 })
        );
        assert_eq!(harness.app.state.tabs.len(), 1, "nothing closed yet");

        harness
            .app
            .state
            .pending
            .push(crate::state::Action::SaveAndCloseTab(0));
        harness.frame(Vec::new());
        assert_eq!(harness.app.state.tabs.len(), 0);
        assert_eq!(
            std::fs::read_to_string(harness.root.join("a.rs")).unwrap(),
            "xone\n",
            "save and close writes first"
        );
    }

    #[test]
    fn closing_the_window_with_edits_is_cancelled_and_asked_about() {
        let mut harness = Harness::new(&[("a.rs", "one\n")]);
        harness.type_into("a.rs", "x");

        let output = harness.frame_with(Vec::new(), vec![egui::ViewportEvent::Close]);
        let commands = harness.commands(&output);
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose)),
            "the close must be cancelled, got {commands:?}"
        );
        assert_eq!(harness.app.state.prompt, Some(crate::state::Prompt::Quit));
    }

    #[test]
    fn closing_the_window_without_edits_is_allowed() {
        let mut harness = Harness::new(&[("a.rs", "one\n")]);
        let output = harness.frame_with(Vec::new(), vec![egui::ViewportEvent::Close]);
        let commands = harness.commands(&output);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose)),
            "nothing to save, the window may close: {commands:?}"
        );
        assert!(harness.app.state.prompt.is_none());
    }

    #[test]
    fn ctrl_shift_m_toggles_the_preview() {
        let mut harness = Harness::new(&[("README.md", "# Title\n")]);
        assert!(!harness.app.state.preview_open, "off by default");

        harness.frame(vec![egui::Event::Key {
            key: egui::Key::M,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        }]);
        assert!(harness.app.state.preview_open);
    }

    #[test]
    fn the_preview_takes_the_text_of_a_markdown_tab() {
        let mut harness = Harness::new(&[("README.md", "# Title\n\nbody\n")]);
        harness.app.state.preview_open = true;
        harness.frame(Vec::new());

        assert_eq!(
            harness.app.state.preview.shown(),
            "# Title\n\nbody\n",
            "the first frame feeds the preview"
        );
    }

    #[test]
    fn turning_the_preview_on_for_rust_draws_the_source_alone() {
        let mut harness = Harness::new(&[("a.rs", "fn main() {}\n")]);
        harness.app.state.preview_open = true;
        // The panel must not split for a file it cannot render, and must not
        // panic either.
        harness.frame(Vec::new());
        harness.frame(Vec::new());
        assert!(harness.app.state.preview.shown().is_empty());
    }

    #[test]
    fn a_watcher_sees_a_file_changed_by_someone_else() {
        // End to end: a real watcher on the real directory, no hand-made events.
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.rs"), "one\n").unwrap();
        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("a.rs"));

        std::thread::sleep(std::time::Duration::from_millis(200));
        std::fs::write(project.path().join("a.rs"), "two\n").unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            app.state.poll_fs_events();
            let text = app
                .state
                .buffers
                .values()
                .next()
                .map(|buffer| buffer.text().to_owned())
                .unwrap_or_default();
            if text == "two\n" || std::time::Instant::now() > deadline {
                assert_eq!(text, "two\n", "the watcher never reported the change");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod quick_open_and_theme_tests {
    use super::*;

    fn app_with(files: &[(&str, &str)]) -> (App, tempfile::TempDir, tempfile::TempDir) {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(project.path().join(name), text).unwrap();
        }
        let app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        (app, project, config)
    }

    fn frame(
        ctx: &egui::Context,
        app: &mut App,
        time: &mut f64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        *time += 0.1;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(900.0, 700.0),
            )),
            time: Some(*time),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.draw(ui));
        output.textures_delta.clear();
        output
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn ctrl_p_opens_the_field_and_enter_opens_the_file() {
        let (mut app, _project, _config) = app_with(&[("alpha.rs", "a\n"), ("beta.rs", "b\n")]);
        let ctx = egui::Context::default();
        let mut time = 0.0;
        frame(&ctx, &mut app, &mut time, Vec::new());
        frame(&ctx, &mut app, &mut time, Vec::new());
        assert!(app.state.tabs.is_empty());

        frame(
            &ctx,
            &mut app,
            &mut time,
            vec![key(egui::Key::P, egui::Modifiers::CTRL)],
        );
        assert!(app.state.quick_open.open, "the field is up");

        // The query narrows the list down to beta.rs.
        frame(
            &ctx,
            &mut app,
            &mut time,
            vec![egui::Event::Text("beta".to_owned())],
        );
        assert_eq!(app.state.quick_open.query, "beta");

        frame(
            &ctx,
            &mut app,
            &mut time,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        frame(&ctx, &mut app, &mut time, Vec::new());

        assert!(!app.state.quick_open.open, "the field closes after a pick");
        let open: Vec<_> = app
            .state
            .tabs
            .items()
            .iter()
            .map(|item| item.path.clone())
            .collect();
        assert_eq!(open, vec![std::path::PathBuf::from("beta.rs")]);
    }

    #[test]
    fn escape_closes_the_field_without_opening_anything() {
        let (mut app, _project, _config) = app_with(&[("alpha.rs", "a\n")]);
        let ctx = egui::Context::default();
        let mut time = 0.0;
        frame(&ctx, &mut app, &mut time, Vec::new());
        frame(
            &ctx,
            &mut app,
            &mut time,
            vec![key(egui::Key::P, egui::Modifiers::CTRL)],
        );
        frame(
            &ctx,
            &mut app,
            &mut time,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        frame(&ctx, &mut app, &mut time, Vec::new());

        assert!(!app.state.quick_open.open);
        assert!(app.state.tabs.is_empty());
    }

    #[test]
    fn the_light_theme_keeps_keywords_readable() {
        use crate::editor::view::Palette;
        use ide_core::Theme;
        use ide_syntax::TokenKind;

        let dark = Palette::for_theme(Theme::Dark);
        let light = Palette::for_theme(Theme::Light);
        let keyword_dark = dark.token(TokenKind::Keyword);
        let keyword_light = light.token(TokenKind::Keyword);

        assert_ne!(keyword_dark, keyword_light, "the schemes differ");
        // On a light background the text has to be darker than the paper.
        let brightness = u32::from(keyword_light.r())
            + u32::from(keyword_light.g())
            + u32::from(keyword_light.b());
        assert!(
            brightness < 400,
            "light keyword {keyword_light:?} is washed out"
        );
        assert_eq!(light.foreground, light.token(TokenKind::Operator).clone());
    }

    #[test]
    fn a_tab_whose_file_is_gone_is_not_restored() {
        let (mut app, project, _config) = app_with(&[("here.rs", "a\n")]);
        let workspace = project.path().join(".andy-ide");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            workspace.join("workspace.json"),
            r#"{"tabs":["here.rs","gone.rs"],"active":1,"layout":null}"#,
        )
        .unwrap();

        app.state.open_project(project.path());

        let open: Vec<_> = app
            .state
            .tabs
            .items()
            .iter()
            .map(|item| item.path.clone())
            .collect();
        assert_eq!(open, vec![std::path::PathBuf::from("here.rs")]);
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;
    use crate::lsp::Outcome;
    use lsp_types::{Location, Position, Range};

    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, App) {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = project.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }
        let app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        (project, config, app)
    }

    #[test]
    fn f12_in_a_file_no_server_understands_explains_itself() {
        let (_project, _config, mut app) = project(&[("notes.txt", "just text\n")]);
        app.state.open_file(std::path::Path::new("notes.txt"));
        app.state.goto_definition();
        assert_eq!(app.state.lsp_status, "no language server for notes.txt");
        assert_eq!(app.state.status, "no language server for notes.txt");
        assert!(app.state.lsp.is_none(), "no server was started");
    }

    #[test]
    fn a_definition_opens_its_file_at_the_right_line() {
        let (project, _config, mut app) = project(&[
            ("a.rs", "fn a() {}\n"),
            ("b/c.rs", "one\ntwo\nthree\nfour\n"),
        ]);
        app.state.open_file(std::path::Path::new("a.rs"));

        let target = project.path().join("b/c.rs");
        let uri = ide_lsp::path_to_uri(&target).expect("a file uri");
        app.state.apply_outcome(Outcome::Definitions {
            from: std::path::PathBuf::from("a.rs"),
            locations: vec![Location {
                uri,
                range: Range {
                    start: Position {
                        line: 2,
                        character: 3,
                    },
                    end: Position {
                        line: 2,
                        character: 8,
                    },
                },
            }],
        });

        let path = app.state.tabs.items()[app.state.tabs.active().unwrap()]
            .path
            .clone();
        assert_eq!(
            path,
            std::path::PathBuf::from("b/c.rs"),
            "the target opened"
        );
        let buffer = app.state.active_buffer().expect("a buffer");
        assert_eq!(buffer.line_col(), (2, 3), "the caret is at the definition");
        assert_eq!(buffer.line_text(2), "three");
    }

    #[test]
    fn a_definition_outside_the_project_opens_its_file() {
        let (project, _config, mut app) = project(&[("a.rs", "fn a() {}\n")]);
        app.state.open_file(std::path::Path::new("a.rs"));

        let outside = tempfile::tempdir().unwrap();
        let far = outside.path().join("other.rs");
        std::fs::write(&far, "alpha\nbeta\n").unwrap();
        let uri = ide_lsp::path_to_uri(&far).unwrap();
        app.state.apply_outcome(Outcome::Definitions {
            from: project.path().join("a.rs"),
            locations: vec![Location {
                uri,
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 0,
                        character: 1,
                    },
                },
            }],
        });

        assert!(
            app.state.status.contains("outside the project"),
            "{}",
            app.state.status
        );
        assert_eq!(
            app.state.tabs.items().len(),
            2,
            "the external file did not open"
        );
        let buffer = app.state.active_buffer().expect("a buffer");
        assert_eq!(buffer.path, far, "the tab holds the absolute path");
        assert_eq!(buffer.text(), "alpha\nbeta\n", "the code was not read");
    }

    #[test]
    fn diagnostics_are_kept_per_file_and_cleared_when_the_server_is_quiet() {
        let (project, _config, mut app) = project(&[("a.rs", "fn a() {}\n")]);
        app.state.open_file(std::path::Path::new("a.rs"));
        let absolute = project.path().join("a.rs");
        let uri = ide_lsp::path_to_uri(&absolute).unwrap();

        app.state.apply_outcome(Outcome::Diagnostics {
            path: ide_lsp::uri_to_path(&uri).unwrap(),
            lines: vec![crate::lsp::DiagnosticLine {
                line: 2,
                message: "unused variable".to_owned(),
            }],
        });
        assert_eq!(app.state.diagnostics.len(), 1);
        let (path, lines) = app.state.diagnostics.iter().next().unwrap();
        assert_eq!(path, &std::path::PathBuf::from("a.rs"), "stored relative");
        assert_eq!(lines[0].line, 2);

        app.state.apply_outcome(Outcome::Diagnostics {
            path: ide_lsp::uri_to_path(&uri).unwrap(),
            lines: Vec::new(),
        });
        assert!(app.state.diagnostics.is_empty(), "a quiet file is dropped");
    }

    #[test]
    fn a_failed_server_is_a_status_line_not_a_crash() {
        let (_project, _config, mut app) = project(&[("a.rs", "fn a() {}\n")]);
        app.state.open_file(std::path::Path::new("a.rs"));
        app.state.apply_outcome(Outcome::Failed(
            "rust-analyzer did not start: not found".to_owned(),
        ));
        assert!(app.state.status.contains("did not start"));
        assert_eq!(app.state.active_buffer().unwrap().line_col(), (0, 0));
    }
}

/// Real language servers are heavy and share the cargo cache, so the tests that
/// start one take this lock: they queue instead of starving each other and each
/// other's timeouts.
#[cfg(test)]
pub(crate) static REAL_SERVER: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The whole chain with the real server: a caret in the editor, F12, the worker
/// thread, rust-analyzer, and the answer opening the file at the declaration.
///
/// It needs `rust-analyzer` installed and a minute of patience on a cold start;
/// without the server the check says so and passes, because the editor must not
/// depend on one.
#[cfg(test)]
mod real_navigation_tests {
    use super::*;

    const SOURCE: &str = "fn helper() -> u8 {\n    7\n}\n\nfn main() {\n    let value = helper();\n    println!(\"{value}\");\n}\n";

    #[test]
    fn f12_finds_the_definition_with_rust_analyzer() {
        let _queue = crate::app::REAL_SERVER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(server) = ide_lsp::server_for(std::path::Path::new("src/main.rs")) else {
            println!("no server is known for Rust: the check was skipped");
            return;
        };
        if !ide_lsp::is_installed(server) {
            println!("{} is not installed: the check was skipped", server.name);
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("src/main.rs"), SOURCE).unwrap();

        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("src/main.rs"));

        let call = SOURCE.rfind("helper()").expect("the call");
        let buffer = app.state.buffers.values_mut().next().expect("a buffer");
        buffer.set_caret_byte(call);
        assert_eq!(buffer.line_col(), (5, 16), "the caret is on the call");
        assert!(buffer.line_text(5).trim_start().starts_with("let value"));

        app.state.goto_definition();
        assert!(
            app.state.lsp.is_some(),
            "the server started: {}",
            app.state.lsp_status
        );

        // A cold server may answer nothing at first; the worker retries by itself,
        // but the editor also has to be patient here.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            app.state.poll_lsp();
            let at_definition = app
                .state
                .active_buffer()
                .map(|buffer| buffer.line_col() == (0, 3))
                .unwrap_or(false);
            if at_definition || std::time::Instant::now() > deadline {
                break;
            }
            if !app.state.status.contains("looking for") {
                // Nothing found on this attempt: ask again until the deadline.
                app.state.goto_definition();
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        let buffer = app.state.active_buffer().expect("a buffer");
        assert_eq!(
            buffer.line_col(),
            (0, 3),
            "the caret is on the declaration of `helper` (status: {})",
            app.state.status
        );
        assert!(buffer.line_text(0).starts_with("fn helper"));
        app.state.lsp.take();
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;
    use crate::lsp::Outcome;
    use lsp_types::{Location, Position, Range};

    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, App) {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = project.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }
        let app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        (project, config, app)
    }

    fn at(root: &std::path::Path, name: &str, line: u32, character: u32) -> Location {
        Location {
            uri: ide_lsp::path_to_uri(&root.join(name)).expect("a uri"),
            range: Range {
                start: Position { line, character },
                end: Position {
                    line,
                    character: character + 1,
                },
            },
        }
    }

    #[test]
    fn references_are_ordered_merged_and_made_relative() {
        let (project, _config, mut app) = project(&[
            ("a.rs", "one\ntwo\n"),
            ("b.rs", "alpha\nbeta\n"),
            ("c/x.rs", "gamma\n"),
        ]);
        app.state.open_file(std::path::Path::new("a.rs"));

        // Out of order on purpose, with the same place twice and one file the
        // project does not contain.
        let outside = tempfile::tempdir().unwrap();
        let locations = vec![
            at(project.path(), "b.rs", 1, 0),
            at(project.path(), "a.rs", 1, 2),
            at(project.path(), "a.rs", 1, 2),
            at(project.path(), "c/x.rs", 0, 0),
            Location {
                uri: ide_lsp::path_to_uri(&outside.path().join("far.rs")).unwrap(),
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 0,
                        character: 1,
                    },
                },
            },
        ];

        app.state.apply_outcome(Outcome::References {
            from: std::path::PathBuf::from("a.rs"),
            locations,
        });

        let hits = &app.state.results.hits;
        assert!(app.state.results.open, "the list did not open");
        assert_eq!(
            hits.len(),
            3,
            "the duplicate or the outside file got in: {hits:?}"
        );
        let paths: Vec<String> = hits
            .iter()
            .map(|hit| hit.path.display().to_string())
            .collect();
        assert_eq!(paths, vec!["a.rs", "b.rs", "c/x.rs"], "not ordered by file");
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].character, 2, "the column is kept");
        assert_eq!(hits[0].text, "two", "the preview is the line of the hit");
        assert_eq!(hits[1].text, "beta");
        assert_eq!(hits[2].text, "gamma");
        assert!(
            app.state.status.contains("3 references"),
            "status was {:?}",
            app.state.status
        );
    }

    #[test]
    fn the_preview_of_an_edited_buffer_shows_the_edits() {
        let (project, _config, mut app) = project(&[("a.rs", "one\ntwo\n")]);
        app.state.open_file(std::path::Path::new("a.rs"));

        let id = app
            .state
            .tabs
            .items()
            .iter()
            .find(|item| item.path == std::path::Path::new("a.rs"))
            .map(|item| item.buffer_id)
            .expect("the file is open");
        let buffer = app.state.buffers.get_mut(&id).unwrap();
        buffer.set_caret(0);
        buffer.insert_text("edited ");

        app.state.apply_outcome(Outcome::References {
            from: std::path::PathBuf::from("a.rs"),
            locations: vec![at(project.path(), "a.rs", 0, 0)],
        });

        assert_eq!(
            app.state.results.hits[0].text, "edited one",
            "the preview came from disk, not from the buffer"
        );
    }

    #[test]
    fn an_empty_answer_says_so_and_opens_nothing() {
        let (_project, _config, mut app) = project(&[("a.rs", "one\n")]);
        app.state.open_file(std::path::Path::new("a.rs"));

        app.state.apply_outcome(Outcome::References {
            from: std::path::PathBuf::from("a.rs"),
            locations: Vec::new(),
        });

        assert!(!app.state.results.open, "an empty list was shown");
        assert!(
            app.state.status.contains("no references"),
            "status was {:?}",
            app.state.status
        );
    }
}

/// `Shift+F12` through the whole chain with the real server.
#[cfg(test)]
mod real_reference_tests {
    use super::*;

    const SOURCE: &str = "fn helper() -> u8 {\n    7\n}\n\nfn main() {\n    let value = helper();\n    println!(\"{value}\");\n}\n";

    #[test]
    fn shift_f12_lists_the_uses_with_rust_analyzer() {
        let _queue = crate::app::REAL_SERVER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(server) = ide_lsp::server_for(std::path::Path::new("src/main.rs")) else {
            println!("no server is known for Rust: the check was skipped");
            return;
        };
        if !ide_lsp::is_installed(server) {
            println!("{} is not installed: the check was skipped", server.name);
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("src/main.rs"), SOURCE).unwrap();

        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("src/main.rs"));

        // The caret on the declaration of `helper`, which has one use.
        let declaration = SOURCE.find("helper").expect("the declaration") + 1;
        let buffer = app.state.buffers.values_mut().next().expect("a buffer");
        buffer.set_caret_byte(declaration);

        app.state.find_references();
        assert!(app.state.lsp.is_some(), "the server started");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while !app.state.results.open && std::time::Instant::now() < deadline {
            app.state.poll_lsp();
            if !app.state.status.contains("looking for") && !app.state.results.open {
                // A cold server answers nothing at first: ask again.
                app.state.find_references();
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        let hits = &app.state.results.hits;
        assert!(
            !hits.is_empty(),
            "rust-analyzer listed no uses (status: {})",
            app.state.status
        );
        let lines: Vec<u32> = hits.iter().map(|hit| hit.line).collect();
        assert!(
            lines.contains(&0),
            "the declaration itself is missing from {lines:?}"
        );
        assert!(
            lines.contains(&5),
            "the call on line 6 is missing from {lines:?}"
        );
        assert!(
            hits.iter().all(|hit| hit.path.ends_with("src/main.rs")),
            "a hit points outside the file: {hits:?}"
        );
        assert!(
            hits.iter().all(|hit| !hit.text.trim().is_empty()),
            "a preview is empty: {hits:?}"
        );
        app.state.lsp.take();
    }
}

/// The outline (`Ctrl+F12`) and the project symbols (`Ctrl+Shift+F12`) before a
/// server is involved: what the rows look like, and what a missing server says.
#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::lsp::Outcome;
    use crate::ui::results::SymbolKind;
    use lsp_types::{DocumentSymbol, Position, Range, SymbolInformation, SymbolKind as Lsp};
    use std::path::Path;

    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, App) {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = project.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }
        let app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        (project, config, app)
    }

    fn range(line: u32, character: u32) -> Range {
        Range {
            start: Position { line, character },
            end: Position {
                line,
                character: character + 1,
            },
        }
    }

    fn symbol(
        name: &str,
        kind: Lsp,
        line: u32,
        children: Option<Vec<DocumentSymbol>>,
    ) -> DocumentSymbol {
        DocumentSymbol {
            name: name.to_owned(),
            detail: None,
            kind,
            tags: None,
            // Deprecated in the protocol in favour of `tags`; the field is still
            // required by the struct, so a test fixture fills it.
            #[allow(deprecated)]
            deprecated: None,
            range: range(line, 0),
            selection_range: range(line, 4),
            children,
        }
    }

    fn outline() -> Vec<DocumentSymbol> {
        vec![symbol(
            "App",
            Lsp::STRUCT,
            0,
            Some(vec![
                symbol("draw", Lsp::METHOD, 10, None),
                symbol(
                    "Inner",
                    Lsp::STRUCT,
                    20,
                    Some(vec![symbol("new", Lsp::FUNCTION, 25, None)]),
                ),
            ]),
        )]
    }

    #[test]
    fn an_outline_flattens_children_under_their_parent() {
        let (_project, _config, mut app) = project(&[("a.rs", "x\n")]);
        app.state.open_file(std::path::Path::new("a.rs"));
        app.state.apply_outcome(Outcome::Symbols {
            from: "a.rs".into(),
            symbols: outline(),
        });

        let hits = &app.state.results.hits;
        let rows: Vec<(&str, usize, SymbolKind)> = hits
            .iter()
            .map(|hit| (hit.text.as_str(), hit.depth, hit.kind.unwrap()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("App", 0, SymbolKind::Type),
                ("draw", 1, SymbolKind::Method),
                ("Inner", 1, SymbolKind::Type),
                ("new", 2, SymbolKind::Function),
            ],
            "the outline is not in reading order with its nesting"
        );
        assert_eq!(hits[3].line, 25, "a row points at the wrong line");
        assert_eq!(hits[3].character, 4, "a row points at the wrong column");
    }

    #[test]
    fn an_outline_opens_the_list_with_its_title() {
        let (_project, _config, mut app) = project(&[("a.rs", "x\n")]);
        app.state.open_file(std::path::Path::new("a.rs"));
        app.state.apply_outcome(Outcome::Symbols {
            from: "a.rs".into(),
            symbols: outline(),
        });

        assert!(app.state.results.open, "the outline did not open");
        assert_eq!(
            app.state.results.title.as_deref(),
            Some("Symbols in a.rs: 4")
        );
        assert_eq!(app.state.status, "4 symbols in a.rs");
        assert_eq!(app.state.results.hits[0].path, PathBuf::from("a.rs"));
    }

    #[test]
    fn an_empty_outline_says_so_instead_of_opening_a_blank_list() {
        let (_project, _config, mut app) = project(&[("a.rs", "x\n")]);
        app.state.open_file(std::path::Path::new("a.rs"));
        app.state.apply_outcome(Outcome::Symbols {
            from: "a.rs".into(),
            symbols: Vec::new(),
        });

        assert!(!app.state.results.open, "an empty outline opened a list");
        assert_eq!(app.state.status, "no symbols in a.rs");
    }

    #[test]
    fn project_symbols_are_made_relative_and_sorted() {
        let (proj, _config, mut app) = project(&[("a.rs", "x\n"), ("b/c.rs", "y\n")]);
        let outside = tempfile::tempdir().unwrap();
        let far = outside.path().join("far.rs");
        std::fs::write(&far, "z\n").unwrap();

        let information = |path: &std::path::Path, line: u32, name: &str| SymbolInformation {
            name: name.to_owned(),
            kind: Lsp::FUNCTION,
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            location: lsp_types::Location {
                uri: ide_lsp::path_to_uri(path).unwrap(),
                range: range(line, 0),
            },
            container_name: None,
        };
        // The search asks, then an answer comes back: the answer is only taken
        // while the field is up and the query still matches.
        app.state.symbols.open();
        app.state.symbols.query = "i".to_owned();
        app.state.apply_outcome(Outcome::WorkspaceSymbols {
            query: "i".to_owned(),
            symbols: vec![
                information(&far, 0, "far"),
                information(&proj.path().join("b/c.rs"), 3, "second"),
                information(&proj.path().join("a.rs"), 7, "first"),
            ],
        });

        let hits = &app.state.symbols.hits;
        assert_eq!(hits.len(), 3);
        // Sorted by path: the absolute one comes first, as `/tmp/...` sorts
        // before any relative path.
        assert_eq!(
            hits[0].path, far,
            "a symbol outside the project lost its path"
        );
        let paths: Vec<&Path> = hits.iter().map(|hit| hit.path.as_path()).collect();
        assert!(paths.contains(&Path::new("a.rs")), "{paths:?}");
        assert!(paths.contains(&Path::new("b/c.rs")), "{paths:?}");
        assert_eq!(
            app.state.symbols.hits[0].kind,
            Some(crate::ui::results::SymbolKind::Function)
        );
    }

    #[test]
    fn asking_for_symbols_without_a_server_explains_itself() {
        let (_project, _config, mut app) = project(&[("notes.txt", "text\n")]);
        app.state.open_file(std::path::Path::new("notes.txt"));

        app.state.file_symbols();
        assert_eq!(app.state.status, "no language server for notes.txt");
        assert!(
            app.state.lsp.is_none(),
            "a server was started for a text file"
        );
        assert!(!app.state.results.open);

        app.state.open_symbol_search();
        assert_eq!(app.state.status, "no language server for notes.txt");
        assert!(!app.state.symbols.open, "the field opened without a server");
    }

    #[test]
    fn the_symbol_search_with_no_file_open_asks_for_one() {
        let (_project, _config, mut app) = project(&[("a.rs", "x\n")]);
        app.state.open_symbol_search();
        assert_eq!(
            app.state.status,
            "open a file first: its language picks the server"
        );
    }
}

/// The outline and the project symbols through the real server.
#[cfg(test)]
mod real_symbol_tests {
    use super::*;

    const SOURCE: &str = "pub struct Point {\n    pub x: i32,\n    pub y: i32,\n}\n\nimpl Point {\n    pub fn new(x: i32, y: i32) -> Self {\n        Self { x, y }\n    }\n\n    pub fn sum(&self) -> i32 {\n        self.x + self.y\n    }\n}\n\npub const ORIGIN: Point = Point { x: 0, y: 0 };\n\npub fn make() -> Point {\n    Point::new(1, 2)\n}\n";

    fn started_app() -> Option<(tempfile::TempDir, tempfile::TempDir, App)> {
        let server = ide_lsp::server_for(std::path::Path::new("src/lib.rs"))?;
        if !ide_lsp::is_installed(server) {
            println!("{} is not installed: the check was skipped", server.name);
            return None;
        }
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("src/lib.rs"), SOURCE).unwrap();
        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("src/lib.rs"));
        Some((project, config, app))
    }

    /// Polls until the list has something in it, asking again if the server was
    /// still indexing.
    fn wait_for_list(app: &mut App, ask: fn(&mut AppState)) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            app.state.poll_lsp();
            if app.state.results.open {
                return true;
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            if !app.state.status.contains("looking for") {
                ask(&mut app.state);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    #[test]
    fn ctrl_f12_lists_the_symbols_of_the_file() {
        let _queue = crate::app::REAL_SERVER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some((_project, _config, mut app)) = started_app() else {
            return;
        };
        app.state.file_symbols();
        assert!(wait_for_list(&mut app, |state| state.file_symbols()));

        let names: Vec<&str> = app
            .state
            .results
            .hits
            .iter()
            .map(|hit| hit.text.as_str())
            .collect();
        assert!(names.contains(&"Point"), "the struct is missing: {names:?}");
        assert!(names.contains(&"new"), "the method is missing: {names:?}");
        assert!(
            names.contains(&"make"),
            "the function is missing: {names:?}"
        );
        assert!(
            names.contains(&"ORIGIN"),
            "the constant is missing: {names:?}"
        );

        // `new` is nested in the impl block, so it must be drawn deeper than the
        // struct it belongs to.
        let deepest = app
            .state
            .results
            .hits
            .iter()
            .find(|hit| hit.text == "new")
            .map(|hit| hit.depth)
            .unwrap_or(0);
        assert!(deepest > 0, "the method is not nested under its impl");
        println!("outline: {} rows", app.state.results.hits.len());
    }

    #[test]
    fn ctrl_shift_f12_finds_a_symbol_by_name() {
        let _queue = crate::app::REAL_SERVER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some((_project, _config, mut app)) = started_app() else {
            return;
        };
        app.state.open_symbol_search();
        assert!(app.state.symbols.open, "the search did not open");
        app.state.symbols.query = "make".to_owned();

        // Frames drive the debounce and the answer, exactly as the window does.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut now = 0.0;
        loop {
            now += 0.1;
            app.state.poll_symbol_search(now);
            app.state.poll_lsp();
            if !app.state.symbols.hits.is_empty() || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let names: Vec<&str> = app
            .state
            .symbols
            .hits
            .iter()
            .map(|hit| hit.text.as_str())
            .collect();
        assert!(
            names.contains(&"make"),
            "the search found nothing for make: {names:?}"
        );
        assert!(
            app.state
                .symbols
                .hits
                .iter()
                .all(|hit| hit.path.ends_with("src/lib.rs")),
            "a project symbol came from another file"
        );
        println!("search make: {} rows", app.state.symbols.hits.len());
    }
}

/// The hint through the real server: pointer on a call, signature on screen.
#[cfg(test)]
mod real_hover_tests {
    use super::*;

    const SOURCE: &str = "pub struct Point {\n    pub x: i32,\n    pub y: i32,\n}\n\nimpl Point {\n    /// Makes the origin.\n    pub fn new(x: i32, y: i32) -> Self {\n        Self { x, y }\n    }\n}\n\npub fn make() -> Point {\n    Point::new(1, 2)\n}\n";

    #[test]
    fn resting_on_a_call_shows_its_signature() {
        let _queue = crate::app::REAL_SERVER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(server) = ide_lsp::server_for(std::path::Path::new("src/lib.rs")) else {
            println!("no server for rust: the check was skipped");
            return;
        };
        if !ide_lsp::is_installed(server) {
            println!("{} is not installed: the check was skipped", server.name);
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("src/lib.rs"), SOURCE).unwrap();

        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("src/lib.rs"));

        // The pointer rests on `new` in the call on the last line.
        let line = SOURCE
            .lines()
            .position(|line| line.contains("Point::new(1, 2)"))
            .unwrap();
        let column = SOURCE.lines().nth(line).unwrap().find("new").unwrap();
        let spot_line = line as u32;
        let spot_character = column as u32;
        app.state
            .set_hover_for_test(spot_line, spot_character, egui::pos2(300.0, 200.0));

        let start = std::time::Instant::now();
        let mut now = 0.0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            now += 0.1;
            app.state.poll_lsp();
            app.state.track_hover(now);
            if let Some(text) = app.state.hover.text() {
                println!(
                    "hover after {:.1} s: {}",
                    start.elapsed().as_secs_f64(),
                    text.replace('\n', " / ")
                );
                assert!(
                    text.contains("fn new(x: i32, y: i32) -> Self"),
                    "the signature is not in the answer: {text}"
                );
                assert!(
                    text.contains("Makes the origin."),
                    "the documentation is not in the answer: {text}"
                );
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("no hint came back in 120 s");
    }
}

/// The uses of the symbol under the caret, and the rules around them.
#[cfg(test)]
mod highlight_tests {
    use super::*;
    use crate::lsp::Outcome;

    const TEXT: &str = "fn make() -> u8 { 1 }\n\nfn other() -> u8 {\n    make()\n}\n";

    fn app_with_text() -> (tempfile::TempDir, tempfile::TempDir, App) {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.rs"), TEXT).unwrap();
        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("a.rs"));
        (project, config, app)
    }

    fn occurrences(app: &App) -> Vec<(usize, usize)> {
        app.state
            .active_buffer()
            .map(|buffer| buffer.occurrences.clone())
            .unwrap_or_default()
    }

    #[test]
    fn the_use_under_the_caret_stops_the_question_until_it_moves() {
        let (_project, _config, mut app) = app_with_text();
        let caret = TEXT.find("make()").unwrap();
        app.state.active_buffer_mut().unwrap().set_caret_byte(caret);

        assert!(
            app.state.track_highlights(0.0).is_some(),
            "the caret moved and nothing was scheduled"
        );
        // Nothing is asked while the caret is still; no server is running, and one
        // is not started for decoration.
        assert!(app.state.track_highlights(1.0).is_none());
        assert!(
            app.state.lsp.is_none(),
            "a server was started for a highlight"
        );
    }

    #[test]
    fn an_answer_is_turned_into_byte_ranges_of_this_text() {
        let (_project, _config, mut app) = app_with_text();
        let caret = TEXT.rfind("make").unwrap();
        app.state.active_buffer_mut().unwrap().set_caret_byte(caret);
        app.state.apply_outcome(Outcome::Highlights {
            path: std::path::PathBuf::from("a.rs"),
            line: 3,
            character: 4,
            spots: vec![(0, 3, 0, 7), (3, 4, 3, 8)],
        });

        assert_eq!(
            occurrences(&app),
            vec![
                (3, 7),
                (TEXT.rfind("make").unwrap(), TEXT.rfind("make").unwrap() + 4)
            ],
            "the protocol positions were not turned into the bytes of this text"
        );
    }

    #[test]
    fn an_answer_about_another_symbol_is_dropped() {
        let (_project, _config, mut app) = app_with_text();
        let caret = TEXT.find("make()").unwrap();
        app.state.active_buffer_mut().unwrap().set_caret_byte(caret);

        // The caret has moved to `other` while the server was thinking.
        app.state.apply_outcome(Outcome::Highlights {
            path: std::path::PathBuf::from("a.rs"),
            line: 2,
            character: 3,
            spots: vec![(0, 3, 0, 7)],
        });

        assert!(
            occurrences(&app).is_empty(),
            "the uses of the old symbol were painted"
        );
    }
}

/// Highlights through the real server.
#[cfg(test)]
mod real_highlight_tests {
    use super::*;

    const SOURCE: &str = "pub struct Point {\n    pub x: i32,\n    pub y: i32,\n}\n\npub fn make() -> Point {\n    Point { x: 1, y: 2 }\n}\n";

    #[test]
    fn the_uses_of_a_type_are_found_in_the_file() {
        let _queue = crate::app::REAL_SERVER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(server) = ide_lsp::server_for(std::path::Path::new("src/lib.rs")) else {
            println!("no server for rust: the check was skipped");
            return;
        };
        if !ide_lsp::is_installed(server) {
            println!("{} is not installed: the check was skipped", server.name);
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("src/lib.rs"), SOURCE).unwrap();

        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("src/lib.rs"));

        // A server is not started for decoration, so one is started the way the
        // user would start it: by asking for the file's symbols.
        app.state.file_symbols();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline && !app.state.results.open {
            app.state.poll_lsp();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(app.state.results.open, "the server never answered");
        app.state.results.open = false;

        let declaration = SOURCE.find("Point").unwrap();
        app.state
            .active_buffer_mut()
            .unwrap()
            .set_caret_byte(declaration);

        let mut now = 0.0;
        while std::time::Instant::now() < deadline {
            now += 0.1;
            app.state.poll_lsp();
            app.state.track_highlights(now);
            let buffer = app.state.active_buffer().unwrap();
            if buffer.occurrences.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let text = SOURCE;
            let found: Vec<&str> = buffer
                .occurrences
                .iter()
                .map(|(start, end)| &text[*start..*end])
                .collect();
            println!("highlights: {found:?}");
            assert!(
                found.len() >= 2,
                "a type used twice was found {found:?} times"
            );
            assert!(
                found.iter().all(|word| *word == "Point"),
                "a highlight covers something that is not the symbol: {found:?}"
            );
            return;
        }
        panic!("no highlights came back in 120 s");
    }
}

/// Renaming: the field, the answer, and the writes.
#[cfg(test)]
mod rename_tests {
    use super::*;
    use crate::lsp::Outcome;

    const A: &str = "fn alpha() {}\n";
    const B: &str = "fn use_it() { alpha(); }\n";

    fn project_with_two_files() -> (tempfile::TempDir, tempfile::TempDir, App) {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.rs"), A).unwrap();
        std::fs::write(project.path().join("b.rs"), B).unwrap();
        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("a.rs"));
        app.state
            .active_buffer_mut()
            .unwrap()
            .set_caret_byte(A.find("alpha").unwrap());
        (project, config, app)
    }

    /// What rust-analyzer answers for renaming `alpha` in these two files.
    fn answer(app: &mut App, new_name: &str) {
        app.state.apply_outcome(Outcome::Rename {
            new_name: new_name.to_owned(),
            changes: vec![
                (std::path::PathBuf::from("a.rs"), vec![(0, 3, 0, 8)]),
                (
                    std::path::PathBuf::from("b.rs"),
                    vec![(
                        0,
                        B.find("alpha").unwrap() as u32,
                        0,
                        (B.find("alpha").unwrap() + 5) as u32,
                    )],
                ),
            ],
        });
    }

    #[test]
    fn the_field_opens_with_the_word_under_the_caret() {
        let (_project, _config, mut app) = project_with_two_files();
        app.state.rename.begin("alpha");
        assert!(app.state.rename.open);
        assert_eq!(app.state.rename.name, "alpha");
    }

    #[test]
    fn the_answer_is_shown_before_anything_is_written() {
        let (_project, _config, mut app) = project_with_two_files();
        app.state.rename.begin("alpha");
        answer(&mut app, "beta");

        assert!(app.state.rename.preview, "the preview did not appear");
        assert_eq!(app.state.rename.extent(), (2, 2));
        assert!(
            app.state.active_buffer().unwrap().text().contains("alpha"),
            "the text was changed before the user saw the list"
        );
    }

    #[test]
    fn accepting_the_preview_writes_every_file() {
        let (project, _config, mut app) = project_with_two_files();
        app.state.rename.begin("alpha");
        answer(&mut app, "beta");
        app.state.apply_rename();

        assert!(
            app.state
                .active_buffer()
                .unwrap()
                .text()
                .contains("fn beta()")
        );
        assert_eq!(
            std::fs::read_to_string(project.path().join("a.rs")).unwrap(),
            "fn beta() {}\n",
            "the open file was not saved"
        );
        assert_eq!(
            std::fs::read_to_string(project.path().join("b.rs")).unwrap(),
            "fn use_it() { beta(); }\n",
            "the file that was not open was not written"
        );
        assert!(
            app.state.status.contains("renamed 2 uses in 2 files"),
            "the status does not say what happened: {}",
            app.state.status
        );
        assert!(!app.state.rename.open, "the modal stayed open");
    }

    #[test]
    fn an_absolute_answer_reaches_the_open_buffer() {
        // A server answers with URIs, which are absolute; the tab is open under a
        // project-relative path. Both must meet in the same buffer.
        let (project, _config, mut app) = project_with_two_files();
        app.state.rename.begin("alpha");
        let absolute = project.path().join("a.rs");
        app.state.apply_outcome(Outcome::Rename {
            new_name: "beta".to_owned(),
            changes: vec![(absolute.clone(), vec![(0, 3, 0, 8)])],
        });
        assert!(app.state.rename.preview, "{}", app.state.status);

        app.state.apply_rename();

        assert!(
            app.state
                .active_buffer()
                .unwrap()
                .text()
                .contains("fn beta()"),
            "the open buffer still shows the old name"
        );
        assert_eq!(
            std::fs::read_to_string(absolute).unwrap(),
            "fn beta() {}\n",
            "the file on disk was not written"
        );
    }

    #[test]
    fn a_file_that_changed_after_the_preview_stops_the_rename() {
        let (project, _config, mut app) = project_with_two_files();
        app.state.rename.begin("alpha");
        answer(&mut app, "beta");

        // Someone else writes the file while the list is on screen.
        std::fs::write(project.path().join("b.rs"), "fn use_it() {}\n").unwrap();
        app.state.apply_rename();

        assert!(
            app.state.status.contains("changed since the preview"),
            "the rename went ahead: {}",
            app.state.status
        );
        assert_eq!(
            std::fs::read_to_string(project.path().join("a.rs")).unwrap(),
            A,
            "a file was renamed although the preview no longer matched"
        );
    }

    #[test]
    fn without_a_server_the_field_is_not_opened() {
        let (project, _config, mut app) = project_with_two_files();
        std::fs::write(project.path().join("notes.txt"), "text\n").unwrap();
        app.state.open_file(std::path::Path::new("notes.txt"));
        let caret = app.state.active_buffer().unwrap().caret_byte();
        assert_eq!(caret, 0);

        app.state.begin_rename();

        assert!(!app.state.rename.open, "the field opened without a server");
        assert_eq!(app.state.status, "no language server for notes.txt");
    }

    #[test]
    fn on_whitespace_there_is_nothing_to_rename() {
        let (_project, _config, mut app) = project_with_two_files();
        // The second line is empty: the caret has no name to take.
        app.state
            .active_buffer_mut()
            .unwrap()
            .set_caret_byte(A.len());
        app.state.begin_rename();
        assert!(!app.state.rename.open);
        assert_eq!(app.state.status, "no symbol under the caret to rename");
    }
}

/// Renaming through the real server, written to the file on disk.
#[cfg(test)]
mod real_rename_tests {
    use super::*;

    const SOURCE: &str =
        "pub fn alpha() -> u8 {\n    1\n}\n\npub fn caller() -> u8 {\n    alpha()\n}\n";

    #[test]
    fn a_function_is_renamed_everywhere_it_is_used() {
        let _queue = crate::app::REAL_SERVER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(server) = ide_lsp::server_for(std::path::Path::new("src/lib.rs")) else {
            println!("no server for rust: the check was skipped");
            return;
        };
        if !ide_lsp::is_installed(server) {
            println!("{} is not installed: the check was skipped", server.name);
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("src/lib.rs"), SOURCE).unwrap();

        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(std::path::Path::new("src/lib.rs"));
        let declaration = SOURCE.find("alpha").unwrap();
        app.state
            .active_buffer_mut()
            .unwrap()
            .set_caret_byte(declaration);

        // Straight to the field, on a server that has answered nothing yet: that is
        // what a person does after opening a file, and the retries inside the job
        // have to carry it.
        app.state.begin_rename();
        assert!(
            app.state.rename.open,
            "the field did not open: {}",
            app.state.status
        );
        assert_eq!(app.state.rename.name, "alpha");

        app.state.rename.name = "beta".to_owned();
        app.state.submit_rename();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline && !app.state.rename.preview {
            app.state.poll_lsp();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            app.state.rename.preview,
            "no preview came back in 120 s: {}",
            app.state.status
        );
        let (uses, files) = app.state.rename.extent();
        println!("rename preview: {uses} uses in {files} files");
        assert!(uses >= 2, "the use inside `caller` was not listed");

        app.state.apply_rename();
        let written = std::fs::read_to_string(project.path().join("src/lib.rs")).unwrap();
        println!("after rename:\n{written}");
        assert!(
            written.contains("pub fn beta()"),
            "the declaration was not renamed"
        );
        assert!(written.contains("    beta()"), "the use was not renamed");
        assert!(!written.contains("alpha"), "a use of the old name is left");
        assert_eq!(
            app.state.active_buffer().unwrap().text(),
            written,
            "the buffer and the file disagree after the rename"
        );
    }
}

/// Tab labels and the watching of files outside the project.
#[cfg(test)]
mod external_files {
    use super::*;

    #[test]
    fn a_file_in_the_project_is_named_by_its_file_name() {
        let root = std::path::Path::new("/work/project");
        let labels = crate::state::tab_labels(
            &[
                std::path::PathBuf::from("src/main.rs"),
                std::path::PathBuf::from("src/lib.rs"),
            ],
            Some(root),
        );
        assert_eq!(labels, vec!["main.rs", "lib.rs"]);
    }

    #[test]
    fn two_files_of_the_same_name_outside_the_project_are_told_apart() {
        let root = std::path::Path::new("/work/project");
        let first = std::path::Path::new("/home/me/.cargo/registry/src/x/ropey-1.6.1/src/mod.rs");
        let second =
            std::path::Path::new("/home/me/.cargo/registry/src/x/ropey-1.6.1/src/block/mod.rs");
        let labels =
            crate::state::tab_labels(&[first.to_path_buf(), second.to_path_buf()], Some(root));
        let a = labels[0].clone();
        let b = labels[1].clone();
        println!("tabs: {a} | {b}");
        assert_eq!(a, "ropey-1.6.1/src/mod.rs");
        assert_ne!(a, b, "two mod.rs look the same in the strip");
    }

    #[test]
    fn a_change_to_an_open_file_outside_the_project_is_reloaded() {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let external = outside.path().join("dep.rs");
        std::fs::write(&external, "one\n").unwrap();

        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        app.state.open_file(&external);
        assert_eq!(
            app.state.active_buffer().unwrap().text(),
            "one\n",
            "the file was not opened by its absolute path"
        );

        std::fs::write(&external, "two\n").unwrap();
        app.state
            .handle_fs_event(ide_core::FsEvent::Changed(external.clone()));

        assert_eq!(
            app.state.active_buffer().unwrap().text(),
            "two\n",
            "a change to a file outside the project went unnoticed"
        );
        assert!(!app.state.active_buffer().unwrap().is_dirty());
    }
}

/// The terminal tab: a real shell behind it.
#[cfg(test)]
mod the_terminal {
    use std::time::{Duration, Instant};

    fn wait_for(terminal: &crate::ui::terminal::Terminal, needle: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if terminal.contents().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn the_tab_runs_a_shell_and_shows_what_it_prints() {
        let mut terminal = crate::ui::terminal::Terminal::new();
        terminal.ensure_started(80, 24);
        assert_eq!(terminal.error, None, "the shell did not start");
        terminal.write(b"echo terminal-ok\n").unwrap();
        assert!(
            wait_for(&terminal, "terminal-ok"),
            "the shell's answer is not on the screen: {:?}",
            terminal.contents()
        );
    }
}

/// The bottom strip is a row of tabs, not a row of panes.
#[cfg(test)]
mod the_bottom_strip {
    use super::*;

    fn active_tab_in(dock: &DockState<Tab>, path: egui_dock::TabPath) -> Option<Tab> {
        let leaf = dock.leaf(path.node_path()).ok()?;
        leaf.tabs().get(path.tab.0).copied()
    }

    fn tab_count_in_the_problems_leaf(dock: &DockState<Tab>) -> usize {
        let problems = dock.find_tab(&Tab::Problems).expect("a problems tab");
        dock.leaf(problems.node_path())
            .map(|leaf| leaf.tabs().len())
            .unwrap_or(0)
    }

    #[test]
    fn the_panels_share_one_leaf() {
        let dock = default_layout();
        let problems = dock.find_tab(&Tab::Problems).expect("a problems tab");
        let terminal = dock.find_tab(&Tab::Terminal).expect("a terminal tab");
        assert_eq!(
            problems.node_path(),
            terminal.node_path(),
            "the terminal still has a pane of its own"
        );
        assert_eq!(tab_count_in_the_problems_leaf(&dock), 4);
        let git = dock.find_tab(&Tab::Git).expect("a git tab");
        assert_eq!(
            problems.node_path(),
            git.node_path(),
            "the git panel is not in the bottom strip"
        );
    }

    /// The tab that is in front is the one that was in front before the change.
    #[test]
    fn the_problems_stay_in_front() {
        let dock = default_layout();
        assert_eq!(
            active_tab_in(&dock, dock.find_tab(&Tab::Problems).unwrap()),
            Some(Tab::Problems)
        );
    }

    #[test]
    fn a_layout_that_split_the_strip_is_tidied() {
        let mut dock = DockState::new(vec![Tab::Editor]);
        dock.split(
            NodePath::MAIN_ROOT,
            Split::Below,
            0.72,
            Node::leaf(Tab::Problems),
        );
        // The shape this version wrote: the terminal in a pane of its own beside the
        // problems, which is exactly what the change removes.
        let problems = dock.find_tab(&Tab::Problems).unwrap();
        dock.split(
            problems.node_path(),
            Split::Right,
            0.5,
            Node::leaf(Tab::Terminal),
        );
        let tidied = with_terminal(dock);
        let problems = tidied.find_tab(&Tab::Problems).expect("a problems tab");
        let terminal = tidied.find_tab(&Tab::Terminal).expect("a terminal tab");
        assert_eq!(problems.node_path(), terminal.node_path());
        assert_eq!(tidied.iter_all_tabs().count(), 5, "a tab was duplicated");
    }

    /// A layout written before the Git panel existed must gain one, not lose it.
    #[test]
    fn a_layout_without_a_git_panel_gets_one_beside_the_problems() {
        let mut dock = DockState::new(vec![Tab::Editor]);
        dock.split(
            NodePath::MAIN_ROOT,
            Split::Below,
            0.72,
            Node::leaf(Tab::Problems),
        );
        let tidied = with_terminal(dock);
        let problems = tidied.find_tab(&Tab::Problems).expect("a problems tab");
        let git = tidied.find_tab(&Tab::Git).expect("a git tab");
        assert_eq!(problems.node_path(), git.node_path());
        assert_eq!(
            active_tab_in(&tidied, problems),
            Some(Tab::Problems),
            "the layout put something else in front"
        );
    }

    #[test]
    fn a_layout_without_a_terminal_gets_one_beside_the_problems() {
        let mut dock = DockState::new(vec![Tab::Editor]);
        dock.split(
            NodePath::MAIN_ROOT,
            Split::Below,
            0.72,
            Node::leaf(Tab::Problems),
        );
        let tidied = with_terminal(dock);
        let problems = tidied.find_tab(&Tab::Problems).expect("a problems tab");
        let terminal = tidied.find_tab(&Tab::Terminal).expect("a terminal tab");
        assert_eq!(problems.node_path(), terminal.node_path());
        assert_eq!(
            active_tab_in(&tidied, tidied.find_tab(&Tab::Problems).unwrap()),
            Some(Tab::Problems)
        );
        assert_eq!(tidied.iter_all_tabs().count(), 5);
    }
}
