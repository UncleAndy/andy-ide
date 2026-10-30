//! The tab strip above the editor.
//!
//! The strip is drawn by hand rather than by the dock, because the editor keeps
//! its own tab model: the strip has to show dirty markers and offer per-tab
//! closing with the unsaved-changes guard.

use egui::Ui;

use crate::state::{Action, AppState};

/// Draws one button per open file; the active one is highlighted and the dirty
/// ones carry a bullet.
pub fn strip(ui: &mut Ui, state: &mut AppState) {
    let root = state.project.as_ref().map(|project| project.root());
    let paths: Vec<std::path::PathBuf> = state
        .tabs
        .items()
        .iter()
        .map(|item| item.path.clone())
        .collect();
    let labels = crate::state::tab_labels(&paths, root);
    let entries: Vec<(usize, String, bool)> = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| (index, label, state.tabs.dirty(index)))
        .collect();

    let active = state.tabs.active();
    let mut activate = None;
    let mut close = None;

    egui::ScrollArea::horizontal()
        .id_salt("tab_strip")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (index, name, dirty) in &entries {
                    let title = if *dirty {
                        format!("{name} •")
                    } else {
                        name.clone()
                    };
                    let response = ui.selectable_label(active == Some(*index), title);
                    if response.clicked() {
                        activate = Some(*index);
                    }
                    if response.middle_clicked() {
                        close = Some(*index);
                    }
                    if ui.small_button("x").on_hover_text("close").clicked() {
                        close = Some(*index);
                    }
                    ui.add_space(2.0);
                }
            });
        });

    if let Some(index) = activate {
        // Arriving at a tab is arriving at the caret.
        state.focus_tab(index);
    }
    if let Some(index) = close {
        state.pending.push(Action::CloseTab(index));
    }
}
