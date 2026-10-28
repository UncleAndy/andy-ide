//! The Git panel: the branch, the two halves of `git status`, the commit field.
//!
//! A click selects a change, a double click opens the file, Enter shows the diff.
//! Staging and committing are buttons on purpose: they change the repository, and a
//! gesture that changes something should be one the user can see and aim at.

use egui::Ui;
use ide_git::{Area, Change};

use crate::state::{Action, AppState};

/// What the panel was asked to do this frame, applied after the list is drawn.
enum Asked {
    StageSelected,
    UnstageSelected,
    StageAll,
    Commit,
}

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

    let mut asked = None;
    if !state.git.changes.is_empty() {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(state.git.selected.is_some(), egui::Button::new("Stage"))
                .on_hover_text("Put the selected file into the index")
                .clicked()
            {
                asked = Some(Asked::StageSelected);
            }
            if ui
                .add_enabled(state.git.selected.is_some(), egui::Button::new("Unstage"))
                .on_hover_text("Take the selected file out of the index")
                .clicked()
            {
                asked = Some(Asked::UnstageSelected);
            }
            if ui
                .button("Stage all")
                .on_hover_text("Put every change into the index")
                .clicked()
            {
                asked = Some(Asked::StageAll);
            }
        });
    }

    ui.horizontal(|ui| {
        let field = egui::TextEdit::singleline(&mut state.git.message)
            .hint_text("Commit message")
            .desired_width(ui.available_width() - 90.0);
        let response = ui.add(field);
        // Enter in the field commits: the field has the keyboard, so Enter cannot
        // mean "show the diff" here, and a second key to commit would be a mystery.
        let typed_enter =
            response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if ui
            .add_enabled(
                !state.git.message.trim().is_empty(),
                egui::Button::new("Commit"),
            )
            .clicked()
            || typed_enter
        {
            asked = Some(Asked::Commit);
        }
    });

    if let Some(outcome) = &state.git.outcome {
        ui.add_space(2.0);
        ui.weak(outcome.clone());
    }
    ui.separator();

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
                        clicked = Some(index);
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

    if let Some(index) = clicked {
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

    match asked {
        Some(Asked::StageSelected) => state.git.stage_selected(),
        Some(Asked::UnstageSelected) => state.git.unstage_selected(),
        Some(Asked::StageAll) => state.git.stage_all(),
        Some(Asked::Commit) => state.git.commit(),
        None => {}
    }
}
