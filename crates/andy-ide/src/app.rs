//! The application: dock layout, menu bar, status bar.

use std::path::PathBuf;

use egui::{Key, KeyboardShortcut, Modifiers};
use egui_dock::{DockArea, DockState, Node, NodePath, Split, Style};

use crate::state::{AppState, display_name};
use crate::ui::{Tab, Viewer};

/// Font size that maps to zoom factor 1.0; the configured size scales the UI.
const DEFAULT_FONT_SIZE: f32 = 14.0;

/// The window's contents.
pub struct App {
    state: AppState,
    dock: DockState<Tab>,
    zoom_applied: bool,
    /// Frames drawn so far; the dock has no real rectangles before the first one.
    frames: u32,
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
        let dock = state
            .stored_layout()
            .and_then(|layout| serde_json::from_str::<DockState<Tab>>(&layout).ok())
            .unwrap_or_else(default_layout);
        Self {
            state,
            dock,
            zoom_applied: false,
            frames: 0,
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
    dock
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
            ui.ctx().set_zoom_factor(zoom.clamp(0.6, 2.5));
        }

        egui::Panel::top("menu_bar").show(ui, |ui| menu_bar(ui, &mut self.state));
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            crate::ui::status::bar(ui, &self.state);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let mut viewer = Viewer {
                state: &mut self.state,
            };
            DockArea::new(&mut self.dock)
                .style(Style::from_egui(ui.style().as_ref()))
                .show_inside(ui, &mut viewer);
        });

        self.state.apply_pending();
        self.handle_keys(ui);

        self.frames += 1;
        self.state.poll_index();
        // The dock only has real rectangles after a frame, and a layout with
        // NaN rectangles cannot be read back, so the first frame is skipped.
        let layout = if self.frames > 1 {
            serde_json::to_string(&self.dock).ok()
        } else {
            None
        };
        let now = ui.ctx().input(|input| input.time);
        let closing = ui.ctx().input(|input| input.viewport().close_requested());
        self.state.save_workspace(layout, now, closing);
    }
}

impl App {
    fn handle_keys(&mut self, ui: &egui::Ui) {
        let close = KeyboardShortcut::new(Modifiers::CTRL, Key::W);
        let save = KeyboardShortcut::new(Modifiers::CTRL, Key::S);

        let save_pressed = ui.ctx().input_mut(|input| input.consume_shortcut(&save));
        let close_pressed = ui.ctx().input_mut(|input| input.consume_shortcut(&close));

        if save_pressed {
            self.state.save_active();
        }
        if close_pressed && let Some(index) = self.state.tabs.active() {
            self.state.close_tab(index);
        }
    }
}

fn menu_bar(ui: &mut egui::Ui, state: &mut AppState) {
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
    fn default_layout_holds_the_three_panels() {
        let dock = default_layout();
        let tabs: Vec<Tab> = dock.iter_all_tabs().map(|(_, tab)| *tab).collect();
        assert_eq!(tabs.len(), 3);
        assert!(tabs.contains(&Tab::Projects));
        assert!(tabs.contains(&Tab::Editor));
        assert!(tabs.contains(&Tab::Problems));
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
        assert_eq!(back.iter_all_tabs().count(), 3);
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
        assert_eq!(restored.dock.iter_all_tabs().count(), 3);
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
        assert_eq!(app.dock.iter_all_tabs().count(), 3);
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
