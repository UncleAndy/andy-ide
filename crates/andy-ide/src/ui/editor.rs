//! The editor tab: the tab strip and, for now, a placeholder for the text area.

use egui::Ui;

use crate::state::AppState;
use crate::ui::tabs;

/// Draws the tab strip and the active document.
pub fn panel(ui: &mut Ui, state: &mut AppState) {
    tabs::strip(ui, state);
    ui.separator();

    let active = state
        .tabs
        .active()
        .and_then(|index| state.tabs.items().get(index).cloned());

    match active {
        Some(item) => {
            ui.add_space(4.0);
            ui.strong(item.path.display().to_string());
            ui.add_space(8.0);
            ui.weak("The text area, syntax highlighting and caret arrive with the next step.");
        }
        None => {
            ui.centered_and_justified(|ui| {
                ui.weak("Double-click a file in the project tree to open it.");
            });
        }
    }
}
