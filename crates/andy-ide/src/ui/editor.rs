//! The editor tab: the tab strip and the text area of the active document.

use egui::Ui;

use crate::editor::view;
use crate::state::AppState;
use crate::ui::tabs;

/// Draws the tab strip, then the active buffer.
pub fn panel(ui: &mut Ui, state: &mut AppState) {
    tabs::strip(ui, state);
    ui.separator();

    if state.tabs.active().is_none() {
        ui.centered_and_justified(|ui| {
            ui.weak("Double-click a file in the project tree to open it.");
        });
        return;
    }

    let changed = match state.active_buffer_mut() {
        Some(buffer) => view::ui(ui, buffer),
        None => false,
    };
    if changed {
        state.sync_dirty();
    }
}
