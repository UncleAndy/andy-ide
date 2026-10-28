//! The history panel: the commits behind `HEAD`, newest first.
//!
//! A click selects, `Enter` shows what the commit changed, a double click opens the
//! files it touched — the same gestures as the Git panel, because they mean the same
//! thing.

use egui::Ui;

use crate::state::AppState;

pub fn panel(ui: &mut Ui, state: &mut AppState) {
    state.git.refresh(false);

    ui.horizontal(|ui| {
        ui.strong("History");
        if let Some(branch) = state.git.branch() {
            ui.separator();
            ui.weak(branch);
        }
    });
    ui.separator();

    if let Some(error) = &state.git.error {
        ui.add_space(6.0);
        ui.weak(error.clone());
        return;
    }

    if state.git.commits.is_empty() {
        ui.add_space(6.0);
        ui.weak("No commits yet.");
        return;
    }

    if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        state.git.open_selected_commit();
    }

    let mut selected = state.git.commit_selected;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, commit) in state.git.commits.iter().enumerate() {
                let text = format!("{}  {}  ({})", commit.short, commit.summary, commit.author);
                let response = ui.add(
                    egui::Label::new(text)
                        .truncate()
                        .sense(egui::Sense::click()),
                );
                if Some(index) == selected {
                    ui.painter().rect_stroke(
                        response.rect.expand(2.0),
                        2.0,
                        ui.visuals().selection.stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                if response.clicked() {
                    selected = Some(index);
                }
                response.on_hover_text("Enter shows what this commit changed");
            }
        });
    state.git.commit_selected = selected;
}
