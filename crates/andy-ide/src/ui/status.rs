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
        if let Some(warming) = state.warming() {
            ui.separator();
            // A wait, and it says so in words. A spinner was tried here and does not belong: it asks for a
            // repaint every frame, which is more than the egui test harness will do (it stops after sixteen
            // steps), so every check that draws a warming server would fail on the animation.
            ui.label(warming);
        }
        if let Some(line) = state.blame_at_caret() {
            ui.separator();
            let when = jiff::Timestamp::from_second(line.when)
                .map(|at| at.strftime("%Y-%m-%d").to_string())
                .unwrap_or_default();
            ui.label(format!(
                "{} {} {when} — {}",
                line.short, line.author, line.summary
            ));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("{} tabs", state.tabs.len()));
            if let Some((line, col)) = state.caret_position() {
                ui.separator();
                ui.label(format!("{line}:{col}"));
            }
            if let Some(language) = state.active_language() {
                ui.label(language);
                ui.separator();
            }
            if let Some(index) = state.tabs.active()
                && state.tabs.dirty(index)
            {
                ui.label("modified");
            }
        });
    });
}
