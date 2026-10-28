//! The editor tab: the tab strip and the text area of the active document.

use egui::Ui;

use crate::editor::view;
use crate::state::AppState;
use crate::ui::preview;
use crate::ui::tabs;

/// Draws the tab strip, then the active buffer, with the Markdown preview
/// beside it when it is switched on and the file is Markdown.
pub fn panel(ui: &mut Ui, state: &mut AppState) {
    tabs::strip(ui, state);
    ui.separator();

    let Some(_) = state.tabs.active() else {
        ui.centered_and_justified(|ui| {
            ui.weak("Double-click a file in the project tree to open it.");
        });
        return;
    };

    let markdown = state
        .active_buffer()
        .map(|buffer| preview::is_markdown(&buffer.path))
        .unwrap_or(false);

    ui.horizontal(|ui| {
        ui.toggle_value(&mut state.preview_open, "Preview")
            .on_hover_text("Ctrl+Shift+M");
        if !markdown {
            ui.weak("(Markdown only)");
        }
    });

    let split = state.preview_open && markdown;
    if !split {
        let changed = match state.active_buffer_mut() {
            Some(buffer) => view::ui(ui, buffer),
            None => false,
        };
        if changed {
            state.sync_dirty();
        }
        return;
    }

    // The source keeps half the width; the preview takes the other half and
    // follows the text with the delay the preview itself decides on.
    let width = ui.available_width();
    let height = ui.available_height();
    let left = (width * 0.5).max(120.0);
    let now = std::time::Instant::now();
    let text = state
        .active_buffer()
        .map(|buffer| buffer.text().to_owned())
        .unwrap_or_default();

    ui.horizontal_top(|ui| {
        let mut changed = false;
        ui.allocate_ui_with_layout(
            egui::vec2(left, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                if let Some(buffer) = state.active_buffer_mut() {
                    changed = view::ui(ui, buffer);
                }
            },
        );
        if changed {
            state.sync_dirty();
        }
        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2((width - left - 8.0).max(80.0), height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                state.preview.update(&text, now);
                let behind = state.preview.shown() != text;
                ui.horizontal(|ui| {
                    ui.weak("Preview");
                    if behind {
                        // The preview waits for typing to pause, and says so.
                        ui.weak("(updating)");
                    }
                });
                state.preview.show(ui, &mut state.commonmark);
            },
        );
    });
}
