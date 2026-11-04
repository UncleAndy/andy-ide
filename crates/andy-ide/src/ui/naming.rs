//! Writing a name: a new file, a new folder, or a new name for what already stands there.
//!
//! The dialog is only the field and the two buttons. What the name is written for is in the state
//! (`AppState::naming`), and the writing itself happens in `AppState::write_name`, so a check can
//! put a name in and read what landed on disk without a mouse.

use egui::{Id, Key, Modal, Modifiers};

use crate::state::{AppState, NamingKind};

/// Draws the dialog, if a name is being written.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let Some(naming) = state.naming.as_mut() else {
        return;
    };
    let escape = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    if escape {
        state.naming = None;
        return;
    }
    // Enter is the same as the button, and only with a name in the field: a file called nothing is
    // not a file a person meant to make.
    let enter = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter))
        && !naming.text.trim().is_empty();
    let mut written = enter;
    let mut cancelled = false;

    let (title, button, hint) = match naming.kind {
        NamingKind::File => (
            "New file",
            "Create",
            "the name of the file, with its extension",
        ),
        NamingKind::Folder => ("New folder", "Create", "the name of the folder"),
        NamingKind::Rename => ("Rename", "Rename", "the new name"),
    };
    Modal::new(Id::new("andy-ide-naming")).show(ctx, |ui| {
        ui.set_min_width(420.0);
        ui.heading(title);
        ui.label(naming.at.display().to_string());
        ui.add_space(4.0);
        let field = ui.add(
            egui::TextEdit::singleline(&mut naming.text)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
        if !naming.focus {
            naming.focus = true;
            field.request_focus();
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ready = !naming.text.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new(button)).clicked() {
                written = true;
            }
            if ui.button("Cancel").clicked() {
                cancelled = true;
            }
        });
    });

    if cancelled {
        state.naming = None;
        return;
    }
    if written {
        state.write_name();
    }
}
