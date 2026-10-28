//! The modal question the editor asks when it cannot decide alone: unsaved
//! edits against a file that changed on disk, a tab about to be closed, a
//! window about to be closed.
//!
//! Nothing is decided here: the modal only puts an [`Action`] on the queue, and
//! [`AppState::apply_pending`] carries it out after the frame. That keeps the
//! answer testable without a mouse.

use crate::state::{Action, AppState, Prompt, display_name};

/// Draws the pending question, if there is one.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let Some(prompt) = state.prompt.clone() else {
        return;
    };
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::new("andy-ide-prompt")).show(ctx, |ui| {
        ui.set_width(420.0);
        match &prompt {
            Prompt::ExternalChange { path } => {
                ui.heading("Changed on disk");
                ui.label(format!(
                    "{} was changed by something else, and this buffer has edits of its own.",
                    display_name(path)
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Reload from disk").clicked() {
                        answer = Some(Action::ReloadExternal(path.clone()));
                    }
                    if ui.button("Keep my version").clicked() {
                        answer = Some(Action::KeepMine(path.clone()));
                    }
                });
            }
            Prompt::CloseTab { index } => {
                let name = state
                    .tabs
                    .items()
                    .get(*index)
                    .map(|item| display_name(&item.path))
                    .unwrap_or_default();
                ui.heading("Unsaved changes");
                ui.label(format!("{name} has edits that are not saved."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save and close").clicked() {
                        answer = Some(Action::SaveAndCloseTab(*index));
                    }
                    if ui.button("Close without saving").clicked() {
                        answer = Some(Action::ForceCloseTab(*index));
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(Action::CancelPrompt);
                    }
                });
            }
            Prompt::Quit => {
                let unsaved = state
                    .buffers
                    .values()
                    .filter(|buffer| buffer.is_dirty())
                    .count();
                ui.heading("Unsaved changes");
                ui.label(format!("{unsaved} files have edits that are not saved."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save all and quit").clicked() {
                        // Saving alone would leave the window open: the close was
                        // already cancelled by the question.
                        state.pending.push(Action::SaveAll);
                        answer = Some(Action::Quit);
                    }
                    if ui.button("Quit without saving").clicked() {
                        answer = Some(Action::Quit);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(Action::CancelPrompt);
                    }
                });
            }
        }
    });

    let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    if escape {
        state.pending.push(Action::CancelPrompt);
        return;
    }
    if let Some(action) = answer {
        // "Save all and quit" needs the quit too, in that order.
        if prompt == Prompt::Quit && action == Action::SaveAll {
            state.pending.push(Action::SaveAll);
            state.pending.push(Action::Quit);
        } else {
            state.pending.push(action);
        }
    }
    let _ = modal;
}
