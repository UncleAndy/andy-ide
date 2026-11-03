//! The diff panel: one change, line by line, with the markers still on.
//!
//! The same panel answers for a working change and for a commit: a diff is a diff,
//! and two panels that differ only in where the text came from would be two places to
//! look for the same thing. A commit brings its files with it — the list on the left, the diff of
//! the one in front on the right — because a commit is read by walking what it changed, and six
//! file headers in one string are not a list.

use egui::{Color32, RichText, Ui};

use crate::state::AppState;

/// Added and removed lines, in colours that read on both themes.
const ADDED: Color32 = Color32::from_rgb(78, 154, 92);
const REMOVED: Color32 = Color32::from_rgb(196, 84, 84);
const HEADER: Color32 = Color32::from_rgb(120, 160, 210);

/// How much of the pane the list of a commit's files takes: about a third, kept between two bounds
/// so that a deep path is readable and the diff beside it still has room.
const FILES_SHARE: f32 = 0.36;
const FILES_LEAST: f32 = 190.0;
const FILES_MOST: f32 = 460.0;

pub fn panel(ui: &mut Ui, state: &mut AppState) {
    if let Some(error) = &state.git.diff_error {
        ui.add_space(6.0);
        ui.weak(error.clone());
        return;
    }

    let mut picked = None;
    if let Some(diff) = &state.git.diff {
        ui.horizontal(|ui| {
            ui.strong(diff.title.clone());
            if !diff.note.is_empty() {
                ui.separator();
                ui.weak(diff.note.clone());
            }
        });
        ui.separator();

        if diff.files.is_empty() {
            if diff.text.is_empty() {
                ui.add_space(6.0);
                ui.weak("This diff is empty.");
            } else {
                lines(ui, &diff.text, "diff_lines");
            }
        } else {
            // The files the commit touched on one side, the diff of the one in front on the other.
            let room = ui.available_width();
            let height = ui.available_height();
            let list = (room * FILES_SHARE).clamp(FILES_LEAST, FILES_MOST);
            let shown = diff.shown.min(diff.files.len() - 1);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(list, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("commit_files")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for (index, file) in diff.files.iter().enumerate() {
                                    if file_row(ui, index == shown, file).clicked() {
                                        picked = Some(index);
                                    }
                                }
                            });
                    },
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| lines(ui, &diff.files[shown].text, "diff_lines"),
                );
            });
        }
    } else {
        ui.add_space(6.0);
        ui.weak("Pick a change in the Git panel and press Enter to see its diff.");
    }

    if let (Some(index), Some(diff)) = (picked, state.git.diff.as_mut()) {
        diff.shown = index;
    }
}

/// The lines of a diff, coloured by what they do.
fn lines(ui: &mut Ui, text: &str, salt: &str) {
    egui::ScrollArea::both()
        .id_salt(salt)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for line in text.lines() {
                let (colour, text) = colour_of(line);
                ui.label(
                    RichText::new(text)
                        .monospace()
                        .color(colour.unwrap_or(ui.visuals().text_color())),
                );
            }
        });
}

/// One file of a commit: the letter `git status` would print, the path, and how many lines went in
/// and out. The row is the click, as the rows of the changes list are — a list where only the
/// letters answer is a list a person misses.
fn file_row(ui: &mut Ui, selected: bool, file: &ide_git::CommitFile) -> egui::Response {
    let height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if selected {
        ui.painter().rect_filled(
            rect,
            2.0,
            ui.visuals().selection.bg_fill.gamma_multiply(0.35),
        );
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }

    let mut job = egui::text::LayoutJob::default();
    job.append(
        &format!("{}  ", file.kind.letter()),
        0.0,
        egui::TextFormat {
            font_id: egui::TextStyle::Monospace.resolve(ui.style()),
            color: super::git::letter_colour(file.kind),
            ..Default::default()
        },
    );
    let plain = egui::TextFormat {
        font_id: egui::TextStyle::Body.resolve(ui.style()),
        color: ui.visuals().strong_text_color(),
        ..Default::default()
    };
    let dimmed = egui::TextFormat {
        color: ui.visuals().weak_text_color(),
        ..plain.clone()
    };
    let path = file.path.display().to_string();
    match path.rfind('/') {
        Some(at) => {
            job.append(&path[..=at], 0.0, dimmed);
            job.append(&path[at + 1..], 0.0, plain);
        }
        None => job.append(&path, 0.0, plain),
    }
    let small = egui::TextFormat {
        font_id: egui::TextStyle::Small.resolve(ui.style()),
        ..Default::default()
    };
    job.append(
        &format!("  +{}", file.added),
        0.0,
        egui::TextFormat {
            color: ADDED,
            ..small.clone()
        },
    );
    job.append(
        &format!(" −{}", file.removed),
        0.0,
        egui::TextFormat {
            color: REMOVED,
            ..small
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: rect.width() - 12.0,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };

    let room = rect.shrink2(egui::vec2(6.0, 0.0));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(room)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            // Not selectable: a label that takes the pointer for text selection would swallow
            // the click the row is for.
            ui.add(egui::Label::new(job).selectable(false));
        },
    );
    response
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
