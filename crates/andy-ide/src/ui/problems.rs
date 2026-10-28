//! The problems panel: what the language server complains about, one line each,
//! clickable.

use egui::Ui;

use crate::state::{Action, AppState, display_name};

pub fn panel(ui: &mut Ui, state: &mut AppState) {
    let total: usize = state.diagnostics.values().map(Vec::len).sum();

    ui.horizontal(|ui| {
        ui.strong(format!(
            "{total} {}",
            if total == 1 { "problem" } else { "problems" }
        ));
        if !state.lsp_status.is_empty() {
            ui.separator();
            ui.weak(state.lsp_status.clone());
        }
    });
    ui.separator();

    if total == 0 {
        ui.add_space(6.0);
        let hint = if state.lsp.is_some() {
            "The language server has nothing to report."
        } else {
            "Diagnostics arrive from the language server of the project."
        };
        ui.weak(hint);
        return;
    }

    let mut go_to = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (path, lines) in &state.diagnostics {
                for line in lines {
                    let text =
                        format!("{}:{}  {}", display_name(path), line.line + 1, line.message);
                    let response = ui.add(
                        egui::Label::new(text)
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                    if response.clicked() {
                        go_to = Some((path.clone(), line.line));
                    }
                    response.on_hover_text("Go to the line");
                }
            }
        });

    if let Some((path, line)) = go_to {
        state.pending.push(Action::OpenFileAt(path, line));
    }
}
