//! The Git panel: the branch, then the two halves of `git status`, one file a line.
//!
//! A click selects a change and opens the file; Enter shows its diff. The two are on
//! separate gestures because a click is what a person does to look at the code, and
//! the diff is a second thought.

use egui::Ui;
use ide_git::{Area, Change};

use crate::state::{Action, AppState};

pub fn panel(ui: &mut Ui, state: &mut AppState) {
    state.git.refresh(false);

    ui.horizontal(|ui| match state.git.branch() {
        Some(branch) => {
            ui.strong(branch);
            let total = state.git.changes.len();
            ui.separator();
            ui.weak(format!(
                "{total} {}",
                if total == 1 { "change" } else { "changes" }
            ));
        }
        None => {
            ui.strong("Git");
        }
    });
    ui.separator();

    if let Some(error) = &state.git.error {
        ui.add_space(6.0);
        ui.weak(error.clone());
        return;
    }

    if state.git.changes.is_empty() {
        ui.add_space(6.0);
        ui.weak("Nothing has changed since the last commit.");
        return;
    }

    // The panel is in front, so the keyboard is not the editor's: Enter on the
    // selected change is the shortest way to its diff.
    if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        state.git.open_selected_diff();
    }

    let mut clicked = None;
    let mut opened = None;
    let mut selected = state.git.selected;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut index = 0;
            for (area, title) in [
                (Area::Staged, "In the index"),
                (Area::Worktree, "In the work tree"),
            ] {
                let changes: Vec<&Change> = match area {
                    Area::Staged => state.git.staged().collect(),
                    Area::Worktree => state.git.worktree().collect(),
                };
                if changes.is_empty() {
                    continue;
                }
                ui.add_space(4.0);
                ui.weak(title);
                for change in changes {
                    // The whole repository-relative path: in a repository the
                    // directory is the half of the answer that tells two `mod.rs`
                    // apart, and `git status` prints it for the same reason.
                    let text = format!("{}  {}", change.kind.letter(), change.path.display());
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
                        clicked = Some((index, change.path.clone()));
                    }
                    // A single click only selects: it must not move the keyboard to
                    // the editor, or Enter would go to the text and not to the diff.
                    if response.double_clicked() {
                        opened = Some(change.path.clone());
                    }
                    response
                        .on_hover_text("Click to select, double-click to open, Enter for the diff");
                    index += 1;
                }
            }
        });

    if let Some((index, _)) = clicked {
        selected = Some(index);
        // The diff belongs to the row that is selected now; an old one would be
        // answering a question nobody asked.
        state.git.diff = None;
        state.git.diff_error = None;
    }
    state.git.selected = selected;
    if let Some(path) = opened {
        state.pending.push(Action::OpenFile(path));
    }
}
