//! The tab strip above the editor.
//!
//! The strip is drawn by hand rather than by the dock, because the editor keeps
//! its own tab model: the strip has to show dirty markers and offer per-tab
//! closing with the unsaved-changes guard.

use egui::Ui;

use crate::state::{Action, AppState, display_name};

/// Draws one button per open file; the active one is highlighted and the dirty
/// ones carry a bullet.
pub fn strip(ui: &mut Ui, state: &mut AppState) {
    let entries: Vec<(usize, String, bool)> = state
        .tabs
        .items()
        .iter()
        .enumerate()
        .map(|(index, item)| (index, display_name(&item.path), state.tabs.dirty(index)))
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
        state.tabs.set_active(index);
    }
    if let Some(index) = close {
        state.pending.push(Action::CloseTab(index));
    }
}
