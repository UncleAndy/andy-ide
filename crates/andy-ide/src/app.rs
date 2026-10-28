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
}

impl App {
    pub fn new(project: Option<PathBuf>, file: Option<PathBuf>) -> Self {
        let mut state = AppState::new(project, file);
        state.restore_session();
        Self {
            state,
            dock: default_layout(),
            zoom_applied: false,
        }
    }
}

/// Editor in the middle, projects on the left, problems along the bottom.
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
        0.28,
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
    }
}

impl App {
    fn handle_keys(&mut self, ui: &egui::Ui) {
        let close = KeyboardShortcut::new(Modifiers::CTRL, Key::W);
        if ui.ctx().input_mut(|input| input.consume_shortcut(&close))
            && let Some(index) = self.state.tabs.active()
        {
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
        };
        let shapes = draw_frames(&mut app, 1);
        assert!(shapes > 0);
    }
}
