//! The status bar at the bottom of the window.

use egui::Ui;

use crate::state::{AppState, PROCESSING};

/// What a line says when the thing it names is still coming up: the bar gives such a line moving dots.
///
/// Andy: "если получится минимальную анимацию сделать — будет совсем хорошо". Not a spinner: a spinner asks for
/// a repaint every frame, which the check harness will not do — that is written down where the warming line is
/// drawn. Dots ask twice a second, which is the same order as the hint and the list already ask for.
///
/// How long between two steps of the dots.
const DOT_STEP: f64 = 0.5;

/// One dot, two, three, and round again.
pub fn dots(now: f64) -> &'static str {
    match (now / DOT_STEP) as i64 % 3 {
        0 => ".",
        1 => "..",
        _ => "...",
    }
}

/// The line to draw: its own words, with the dots of a process still under way.
pub fn walking(line: &str, now: f64) -> String {
    if line.ends_with(PROCESSING) {
        format!("{line}{}", dots(now))
    } else {
        line.to_owned()
    }
}

/// Project path and the last message on the left, tab count on the right.
pub fn bar(ui: &mut Ui, state: &AppState) {
    ui.horizontal(|ui| {
        match &state.project {
            Some(project) => {
                ui.label(project.root().display().to_string());
            }
            None => {
                ui.weak("no project");
            }
        }
        ui.separator();
        let going = walking(&state.status, ui.input(|input| input.time));
        if going != state.status {
            // The dots have to keep coming while the line stands, so the window is asked for the next step.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(DOT_STEP));
        }
        ui.label(going);
        if let Some(warming) = state.warming() {
            ui.separator();
            // A wait, and it says so in words. A spinner was tried here and does not belong: it asks for a
            // repaint every frame, which is more than the egui test harness will do (it stops after sixteen
            // steps), so every check that draws a warming server would fail on the animation.
            let going = walking(warming, ui.input(|input| input.time));
            if going != warming {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(DOT_STEP));
            }
            ui.label(going);
        }
        if let Some(line) = state.blame_at_caret() {
            ui.separator();
            let when = jiff::Timestamp::from_second(line.when)
                .map(|at| at.strftime("%Y-%m-%d").to_string())
                .unwrap_or_default();
            ui.label(format!(
                "{} {} {when} — {}",
                line.short, line.author, line.summary
            ));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("{} tabs", state.tabs.len()));
            if let Some((line, col)) = state.caret_position() {
                ui.separator();
                ui.label(format!("{line}:{col}"));
            }
            if let Some(language) = state.active_language() {
                ui.label(language);
                ui.separator();
            }
            if let Some(index) = state.tabs.active()
                && state.tabs.dirty(index)
            {
                ui.label("modified");
            }
        });
    });
}

/// The checks of the dots, which are pure: the bar is what draws them.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dots_move_and_come_round() {
        assert_eq!(dots(0.0), ".");
        assert_eq!(dots(0.6), "..");
        assert_eq!(dots(1.1), "...");
        assert_eq!(dots(1.6), ".", "the dots did not come round again");
    }

    #[test]
    fn only_a_line_about_a_process_gets_dots() {
        assert_eq!(
            walking("rust-analyzer processing", 0.0),
            "rust-analyzer processing."
        );
        assert_eq!(
            walking("saved", 0.0),
            "saved",
            "a line about a finished thing was given moving dots"
        );
        assert_eq!(walking("", 0.0), "");
    }
}
