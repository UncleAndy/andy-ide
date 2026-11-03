//! The history panel: the commits behind `HEAD`, newest first.
//!
//! A click selects, the arrow keys walk the list, `Enter` shows what the commit changed — the
//! same gestures as the Git panel, because they mean the same thing.

use egui::{Align, Color32, Layout, RichText, Ui};
use ide_git::{Commit, RefKind, RefMark};

use crate::state::AppState;

/// The short hash, in a colour that reads on both themes.
const HASH: Color32 = Color32::from_rgb(206, 168, 78);
/// Where `HEAD` is, and the three kinds of name that can point at a commit.
const HEAD_CHIP: Color32 = Color32::from_rgb(78, 154, 92);
const LOCAL_CHIP: Color32 = Color32::from_rgb(120, 160, 210);
const REMOTE_CHIP: Color32 = Color32::from_rgb(150, 152, 156);
const TAG_CHIP: Color32 = Color32::from_rgb(198, 120, 221);

/// How many names one row shows before it stops: a commit that is a dozen branches at once is a
/// curiosity, and the chips would leave no room for the message.
const CHIPS_SHOWN: usize = 4;

pub fn panel(ui: &mut Ui, state: &mut AppState) {
    state.git.refresh(false);

    ui.horizontal(|ui| {
        ui.strong("History");
        if let Some(branch) = state.git.branch() {
            ui.separator();
            ui.weak(branch);
        }
    });
    ui.separator();

    body(ui, state);
}

/// The commits themselves, without a heading of their own: the Git panel shows the same list
/// inside its own tab, where the heading would be a second one for the same thing.
pub fn body(ui: &mut Ui, state: &mut AppState) {
    if let Some(error) = &state.git.error {
        ui.add_space(6.0);
        ui.weak(error.clone());
        return;
    }

    if state.git.commits.is_empty() {
        ui.add_space(6.0);
        ui.weak("No commits yet.");
        return;
    }

    // The list is walked with the keys, and the row in front is the one `Enter` asks about.
    // The panel is in front, so the keyboard is not the editor\u2019s \u2014 the same reading of `Enter`
    // the changes list has, and for the same reason: the key that opens what is picked is the
    // shortest way to it.
    let last = state.git.commits.len() - 1;
    let mut walked = false;
    let step = ui.input(|input| {
        if input.key_pressed(egui::Key::ArrowDown) {
            Some(1isize)
        } else if input.key_pressed(egui::Key::ArrowUp) {
            Some(-1)
        } else if input.key_pressed(egui::Key::PageDown) {
            Some(10)
        } else if input.key_pressed(egui::Key::PageUp) {
            Some(-10)
        } else {
            None
        }
    });
    let jump = ui.input(|input| {
        if input.key_pressed(egui::Key::Home) {
            Some(0isize)
        } else if input.key_pressed(egui::Key::End) {
            Some(last as isize)
        } else {
            None
        }
    });

    if let Some(step) = step {
        // Nothing selected yet: the first press takes the first row, whichever way it walks.
        let from = state
            .git
            .commit_selected
            .map_or(0, |index| index as isize + step);
        state.git.commit_selected = Some(from.clamp(0, last as isize) as usize);
        walked = true;
    } else if let Some(jump) = jump {
        state.git.commit_selected = Some(jump.clamp(0, last as isize) as usize);
        walked = true;
    }

    if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        state.git.open_selected_commit();
    }

    let selected = state.git.commit_selected;
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, commit) in state.git.commits.iter().enumerate() {
                let response = commit_row(ui, Some(index) == selected, commit);
                if response.clicked() {
                    clicked = Some(index);
                }
                // A double click is the same wish as `Enter`, and a mouse should not have to reach
                // for the keyboard to open what it already pointed at.
                // The double click is counted after the list is drawn (see `clicked_twice`): a row
                // that is rebuilt every frame is not one widget to egui, so `double_clicked` stays
                // false however fast the two clicks come.
                if walked && Some(index) == selected {
                    // A selection the keys walked to has to be on screen: a row that is selected
                    // out of sight is a list that answers a question nobody can see.
                    ui.scroll_to_rect(response.rect, Some(Align::Center));
                }
            }
        });
    if let Some(index) = clicked {
        state.git.commit_selected = Some(index);
        let id = state.git.commits[index].id.clone();
        let now = ui.input(|input| input.time);
        if state.git.clicked_twice(&id, now) {
            state.git.open_selected_commit();
        }
    }
}

/// One commit, in one line: the short hash, the names that point at it, the message, and the day
/// at the right edge.
///
/// A row is the click, so the whole width of it selects; the rest of what is known about the
/// commit — the full id, the author — is on hover, where a name that is not needed to read the
/// list can be found without making every row wider.
fn commit_row(ui: &mut Ui, selected: bool, commit: &Commit) -> egui::Response {
    let height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    // The row keeps its identity from frame to frame: a double click is counted per widget, and an
    // automatic id that shifts with the frame count makes two clicks on one row two clicks on two
    // different widgets — which is exactly why a double click on a commit opened nothing.
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("commit_row", &commit.id)),
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

    let room = rect.shrink2(egui::vec2(6.0, 0.0));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(room)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            // The day is laid out first, from the right: a message cut where the row ends must not
            // push the day out of the panel, and a row is one line, so something has to give.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(when(commit.when)).small().weak())
                        .selectable(false),
                );
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&commit.short).monospace().color(HASH))
                            .selectable(false),
                    );
                    for mark in commit.refs.iter().take(CHIPS_SHOWN) {
                        chip(ui, mark);
                    }
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(RichText::new(&commit.summary))
                            .selectable(false)
                            .truncate(),
                    );
                });
            });
        },
    );

    response.on_hover_text(format!(
        "{} · {} · {}\nArrow keys walk the list, Enter or a double click shows what it changed",
        commit.id,
        commit.author,
        when(commit.when)
    ))
}

/// One name that points at a commit, as a small plate: `HEAD`, `main`, `origin/main`, `tag: v1.0`.
///
/// The four kinds are told apart by colour, because a list where `main` and `origin/main` look
/// alike says nothing about where the work is and where the remote is.
fn chip(ui: &mut Ui, mark: &RefMark) {
    let colour = match mark.kind {
        RefKind::Head => HEAD_CHIP,
        RefKind::Local => LOCAL_CHIP,
        RefKind::Remote => REMOTE_CHIP,
        RefKind::Tag => TAG_CHIP,
    };
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.22))
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(&mark.name)
                        .monospace()
                        .size(10.0)
                        .color(colour),
                )
                .selectable(false),
            );
        });
}

/// When a commit was written, as a person reads it: the day and the time, and the year only when
/// it is not this one — a week's work does not need the year on every row.
fn when(seconds: i64) -> String {
    let Ok(at) = jiff::Timestamp::from_second(seconds) else {
        return String::new();
    };
    let at = at.to_zoned(jiff::tz::TimeZone::system());
    let year = at.strftime("%Y").to_string();
    let this_year = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y")
        .to_string();
    if year == this_year {
        at.strftime("%d %b %H:%M").to_string()
    } else {
        at.strftime("%d %b %Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::when;

    #[test]
    fn a_commit_is_dated_the_way_a_person_reads_it() {
        // 2026-11-03 14:22 UTC, whatever the machine's zone: the year of the sample is the
        // machine's own, so the shape is what is checked, not the digits.
        let text = when(1_791_632_520);
        assert!(
            text.contains(':') && text.len() <= 16,
            "a date is not a date: {text}"
        );
        assert_eq!(when(i64::MAX), "", "an impossible time was drawn as a date");
    }
}
