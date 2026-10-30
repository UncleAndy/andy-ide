//! The find-and-replace bar of the editor.
//!
//! It stands above the text and owns four keys while it is open — Enter, Shift+Enter, Escape
//! and Tab — and reads them **before** its own fields are drawn: a text field that takes the
//! focus takes the Enter and the Escape for itself, and this bar means them for itself.

use egui::{Key, Modifiers, Ui};

use crate::state::AppState;
use crate::ui::search::Focus::{Find, Replace};

/// Which field has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Find,
    Replace,
}

/// What the bar is doing: whether it is open, what it looks for, and what it writes.
#[derive(Debug, Default)]
pub struct Search {
    pub open: bool,
    /// The replacement field is shown as well (`Ctrl+H` asked for it).
    pub replace: bool,
    pub needle: ide_buffer::Needle,
    pub replacement: String,
    /// The field the keyboard is in.
    pub field: Focus,
    /// The field that should take the focus on this frame.
    pub focus: bool,
}

/// Opens the bar over the file in front, and looks for what it already holds.
pub fn open(state: &mut AppState, replace: bool) {
    state.search.open = true;
    // What the bar shows is what the key asked for, not what the last visit left behind: a
    // `Ctrl+H` that was closed must not leave the replacement field in a later `Ctrl+F`.
    state.search.replace = replace;
    state.search.field = Find;
    state.search.focus = true;
    recount(state);
}

/// Closes the bar and takes its highlight away.
pub fn close(state: &mut AppState) {
    state.search.open = false;
    if let Some(buffer) = state.active_buffer_mut() {
        buffer.end_search();
    }
}

/// Looks for what the bar holds, and takes the caret to the nearest match.
pub fn recount(state: &mut AppState) {
    let needle = state.search.needle.clone();
    let Some(buffer) = state.active_buffer_mut() else {
        return;
    };
    buffer.search(&needle);
    buffer.search_nearest();
}

/// Walks to the next match, or to the one before it.
pub fn step(state: &mut AppState, forward: bool) {
    if let Some(buffer) = state.active_buffer_mut() {
        buffer.search_step(forward);
    }
}

/// Replaces the match the caret stands in, and then goes to the next one.
pub fn replace_here(state: &mut AppState) {
    let replacement = state.search.replacement.clone();
    let changed = match state.active_buffer_mut() {
        Some(buffer) => buffer.replace_at_caret(&replacement),
        None => false,
    };
    if changed {
        state.sync_dirty();
    }
}

/// Replaces every match, as one step of the history.
pub fn replace_everywhere(state: &mut AppState) {
    let replacement = state.search.replacement.clone();
    let changed = match state.active_buffer_mut() {
        Some(buffer) => buffer.replace_all(&replacement) > 0,
        None => false,
    };
    if changed {
        state.sync_dirty();
    }
}

/// Draws the bar.
pub fn bar(ui: &mut Ui, state: &mut AppState) {
    // Read before the fields are drawn: a focused field would swallow both.
    let keys = ui.input_mut(|input| {
        let previous = input.consume_key(Modifiers::SHIFT, Key::Enter);
        let next = input.consume_key(Modifiers::NONE, Key::Enter);
        let close = input.consume_key(Modifiers::NONE, Key::Escape);
        let swap = input.consume_key(Modifiers::SHIFT, Key::Tab);
        let jump = input.consume_key(Modifiers::NONE, Key::Tab);
        (previous, next, close, swap || jump)
    });
    let (previous, next, close_bar, swap) = keys;

    let mut recount_now = false;
    let mut step_to = None;
    let mut replace_now = false;
    let mut all_now = false;
    let mut close_now = close_bar;

    let (hits, current) = match state.active_buffer() {
        Some(buffer) => (buffer.search_hits.len(), buffer.search_current),
        None => (0, None),
    };

    ui.horizontal(|ui| {
        // Plain text on the buttons: the window's font has the letters and the digits, and a
        // character it does not have comes out as a box.
        ui.label("Find");
        let field = ui.add(
            egui::TextEdit::singleline(&mut state.search.needle.text)
                .desired_width(200.0)
                .hint_text("find"),
        );
        if state.search.focus && state.search.field == Find {
            field.request_focus();
            state.search.focus = false;
        }
        if field.changed() {
            recount_now = true;
        }
        if ui
            .button("<")
            .on_hover_text("previous  (Shift+Enter)")
            .clicked()
        {
            step_to = Some(false);
        }
        if ui.button(">").on_hover_text("next  (Enter)").clicked() {
            step_to = Some(true);
        }
        ui.toggle_value(&mut state.search.needle.case_sensitive, "Aa")
            .on_hover_text("match case");
        ui.toggle_value(&mut state.search.needle.whole_word, "ab|")
            .on_hover_text("whole words only");
        if ui.button("×").on_hover_text("close  (Escape)").clicked() {
            close_now = true;
        }
        if state.search.replace {
            ui.separator();
            ui.label("Replace");
            let field = ui.add(
                egui::TextEdit::singleline(&mut state.search.replacement)
                    .desired_width(200.0)
                    .hint_text("replace"),
            );
            if state.search.focus && state.search.field == Replace {
                field.request_focus();
                state.search.focus = false;
            }
            if ui.button("Replace").clicked() {
                replace_now = true;
            }
            if ui.button("Replace all").clicked() {
                all_now = true;
            }
        }
    });

    // What the bar says: the match the caret is on, of how many.
    ui.horizontal(|ui| {
        if state.search.needle.is_empty() {
            ui.weak("no text to look for");
        } else if hits == 0 {
            ui.weak("no matches");
        } else {
            let at = current.map_or(0, |index| index + 1);
            ui.weak(format!("{at} of {hits}"));
        }
    });

    if swap {
        state.search.field = match state.search.field {
            Find => Replace,
            Replace => Find,
        };
        state.search.focus = true;
    }
    if previous {
        step(state, false);
    }
    if next {
        step(state, true);
    }
    if let Some(forward) = step_to {
        step(state, forward);
    }
    if replace_now {
        replace_here(state);
    }
    if all_now {
        replace_everywhere(state);
    }
    if recount_now {
        recount(state);
    }
    if close_now {
        close(state);
    }
}
