//! The conversation panel: who is talking, what was said, and the question on screen.

use egui::Ui;

use crate::agent::Line;
use crate::state::AppState;

/// The panel.
pub fn panel(ui: &mut Ui, state: &mut AppState) {
    state.agent.pump();
    header(ui, state);
    ui.separator();
    transcript(ui, state);
    ui.separator();
    question(ui, state);
    input(ui, state);
}

/// The agent's name as a menu, and the button that starts and stops it.
fn header(ui: &mut Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        let chosen = state
            .agent
            .chosen
            .clone()
            .unwrap_or_else(|| "no agent".to_owned());
        let mut named = chosen.clone();
        egui::ComboBox::from_id_salt("agent-choice")
            .selected_text(chosen.clone())
            .show_ui(ui, |ui| {
                for agent in &state.agent.registry.agents {
                    ui.selectable_value(&mut named, agent.name.clone(), &agent.name);
                }
            });
        if named != chosen {
            state.agent.stop();
            state.agent.chosen = Some(named);
            state.agent.status = "start the agent when ready".to_owned();
        }

        let running = state.agent.running();
        let label = if running { "Stop" } else { "Start" };
        if ui
            .button(label)
            .on_hover_text("Start or stop the agent process")
            .clicked()
        {
            state.agent.toggle();
        }
        ui.separator();
        ui.weak(&state.agent.status);
    });
}

/// Everything said so far, the newest at the bottom.
fn transcript(ui: &mut Ui, state: &AppState) {
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .max_height(ui.available_height() - 64.0)
        .show(ui, |ui| {
            for line in &state.agent.transcript {
                match line {
                    Line::You(text) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong("You  ");
                            ui.label(text);
                        });
                    }
                    Line::Agent(text) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong("Agent");
                            ui.label(text);
                        });
                    }
                    Line::Tool(text) => {
                        ui.weak(format!("  ↳ {text}"));
                    }
                    Line::Note(text) => {
                        ui.colored_label(egui::Color32::from_rgb(200, 120, 90), text);
                    }
                }
            }
        });
}

/// The permission question, when the agent is waiting on one.
fn question(ui: &mut Ui, state: &mut AppState) {
    let Some((_, request)) = &state.agent.asking else {
        return;
    };
    let title = request.title();
    let options: Vec<(String, String)> = request
        .options
        .iter()
        .map(|option| (option.option_id.clone(), option.name.clone()))
        .collect();

    ui.horizontal_wrapped(|ui| {
        ui.colored_label(egui::Color32::from_rgb(220, 190, 90), title);
        let mut chosen: Option<Option<String>> = None;
        for (id, name) in options {
            if ui.button(name).clicked() {
                chosen = Some(Some(id));
            }
        }
        if ui.button("Refuse").clicked() {
            chosen = Some(None);
        }
        if let Some(option) = chosen {
            state.agent.answer(option);
        }
    });
}

/// The field, and the button that sends it.
fn input(ui: &mut Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        let busy = state.agent.busy || !state.agent.running();
        // The field stops short of the button: a field that takes the whole width pushes
        // the button out of the panel, and a button nobody can see is a button nobody
        // can press.
        let width = (ui.available_width() - 90.0).max(80.0);
        let response = ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut state.agent.input)
                .hint_text("Ask the agent…")
                .desired_width(width),
        );
        // Enter in the field sends: the field has the keyboard, so Enter cannot mean
        // anything else here, and a single-line field gives the focus up as it commits.
        let sent = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if sent {
            state.agent.send();
        }
        if ui.add_enabled(!busy, egui::Button::new("Send")).clicked() {
            state.agent.send();
        }
    });
}
