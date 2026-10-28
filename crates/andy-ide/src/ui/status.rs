//! The status bar at the bottom of the window.

use egui::Ui;

use crate::state::AppState;

/// Project path and the last message on the left, tab count on the right.
pub fn bar(ui: &mut Ui, state: &AppState) {
    ui.horizontal(|ui| {
        match &state.project {
            Some(project) => {
                ui.label(project.root().display().to_string());
            }
            None => {
                ui.weak("no project");
            }
        }
        ui.separator();
        ui.label(&state.status);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("{} tabs", state.tabs.len()));
            if let Some(index) = state.tabs.active()
                && state.tabs.dirty(index)
            {
                ui.label("modified");
            }
        });
    });
}
