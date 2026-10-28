//! The Git panel: the branch, then the two halves of `git status`, one file a line.

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

    let mut go_to = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
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
                    if response.clicked() {
                        go_to = Some(change.path.clone());
                    }
                    response.on_hover_text("Open the file");
                }
            }
        });

    if let Some(path) = go_to {
        state.pending.push(Action::OpenFile(path));
    }
}
