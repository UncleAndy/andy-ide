//! The editor tab: the tab strip and the text area of the active document.

use egui::Ui;

use crate::editor::view;
use crate::editor::view::Palette;
use crate::state::AppState;
use crate::ui::preview;
use crate::ui::tabs;

/// Draws the tab strip, then the active buffer — or the Markdown preview, which takes the
/// whole of the editing room when it is switched on for a Markdown file.
pub fn panel(ui: &mut Ui, state: &mut AppState) {
    // The hint under the pointer belongs to this panel: while it is not drawn, there is
    // no text under the pointer to ask about, and a spot left over from an earlier frame
    // would follow the pointer into whichever panel the person is actually using.
    state.editor_drawn = true;
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

    // The Preview switch is for Markdown files: for any other file the row is not
    // drawn at all, and its height belongs to the text.
    if markdown {
        ui.horizontal(|ui| {
            ui.toggle_value(&mut state.preview_open, "Preview")
                .on_hover_text("Ctrl+Shift+M");
        });
    }

    let palette = Palette::for_theme(state.config.theme);
    // The margin marks are read before the buffer is borrowed: both live in the state,
    // and a file being edited has both.
    let marks = match state.active_buffer() {
        Some(buffer) => {
            let path = buffer.path.clone();
            state.git.marks(&path).to_vec()
        }
        None => Vec::new(),
    };
    // The preview stands in the place of the source, across the whole of the editing room:
    // shown beside it, every line appears twice — once as written and once as read — and
    // neither of them has the width to be read in.
    if state.preview_open && markdown {
        let now = std::time::Instant::now();
        let text = state
            .active_buffer()
            .map(|buffer| buffer.text().to_owned())
            .unwrap_or_default();
        // The preview follows the text with the delay it decides on itself, and says so while
        // it waits.
        state.preview.update(&text, now);
        let behind = state.preview.shown() != text;
        ui.horizontal(|ui| {
            ui.weak("Preview");
            if behind {
                ui.weak("(updating)");
            }
        });
        state.preview.show(ui, &mut state.commonmark);
        return;
    }

    let changed = match state.active_buffer_mut() {
        Some(buffer) => view::ui(ui, buffer, palette, &marks),
        None => false,
    };
    if changed {
        state.sync_dirty();
    }
}
