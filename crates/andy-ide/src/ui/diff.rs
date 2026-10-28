//! The diff panel: one file's change, line by line, with the markers still on.

use egui::{Color32, RichText, Ui};

use crate::state::AppState;

/// Added and removed lines, in colours that read on both themes.
const ADDED: Color32 = Color32::from_rgb(78, 154, 92);
const REMOVED: Color32 = Color32::from_rgb(196, 84, 84);
const HEADER: Color32 = Color32::from_rgb(120, 160, 210);

pub fn panel(ui: &mut Ui, state: &mut AppState) {
    if let Some(error) = &state.git.diff_error {
        ui.add_space(6.0);
        ui.weak(error.clone());
        return;
    }

    let Some(diff) = &state.git.diff else {
        ui.add_space(6.0);
        ui.weak("Pick a change in the Git panel and press Enter to see its diff.");
        return;
    };

    ui.horizontal(|ui| {
        ui.strong(diff.path.display().to_string());
        ui.separator();
        ui.weak(match diff.side {
            ide_git::Side::Staged => "against HEAD",
            ide_git::Side::Worktree => "against the index",
        });
    });
    ui.separator();

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for line in diff.lines() {
                let (colour, text) = colour_of(line);
                ui.label(
                    RichText::new(text)
                        .monospace()
                        .color(colour.unwrap_or(ui.visuals().text_color())),
                );
            }
        });
}

/// The colour a diff line is drawn in, and the line itself.
///
/// The header lines keep their own colour so the eye can find where a hunk starts.
fn colour_of(line: &str) -> (Option<Color32>, &str) {
    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
        (Some(HEADER), line)
    } else if line.starts_with('+') {
        (Some(ADDED), line)
    } else if line.starts_with('-') {
        (Some(REMOVED), line)
    } else {
        (None, line)
    }
}
