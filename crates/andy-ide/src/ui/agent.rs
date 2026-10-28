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

/// The command that starts the agent, the menu that fills it in, and the buttons.
///
/// The command is a field and not a label: an ACP agent is any program that speaks the
/// protocol, and the list of them is never complete. The menu writes a command into the
/// field — choosing `Hermes` fills in `hermes acp` — and the field is what runs.
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
            if let Some(spec) = state.agent.registry.get(&named) {
                state.agent.command = ide_agents::quote_command(spec);
            }
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
            if running {
                state.agent.stop();
                state.agent.status = "stopped".to_owned();
            } else {
                state.agent.start();
            }
        }
        if ui
            .add_enabled(!running, egui::Button::new("Remember"))
            .on_hover_text("Write this command into agents.toml")
            .clicked()
        {
            state.agent.remember();
        }
        ui.separator();
        ui.weak(&state.agent.status);
    });
    ui.horizontal(|ui| {
        ui.label("Command");
        let width = (ui.available_width() - 8.0).max(80.0);
        ui.add_enabled(
            !state.agent.running(),
            egui::TextEdit::singleline(&mut state.agent.command)
                .hint_text("hermes acp")
                .desired_width(width),
        )
        .on_hover_text("The program that speaks ACP, with its arguments");
    });
}

/// Everything said so far, the newest at the bottom.
///
/// The answer is read as Markdown while it is still arriving: an answer that talks about
/// code without a code block is an answer nobody wants to read twice, and waiting for the
/// turn to end to render it would mean watching plain text turn into prose.
///
/// The reasoning is folded away behind one marker per stretch: it is how the answer was
/// worked out, not the answer, and a panel that pours it into the same column makes the
/// answer harder to find.
fn transcript(ui: &mut Ui, state: &mut AppState) {
    let lines = state.agent.transcript.clone();
    let height = (ui.available_height() - 64.0).max(60.0);
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .max_height(height)
        .show(ui, |ui| {
            for line in &lines {
                match line {
                    Line::You(text) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong("You  ");
                            ui.label(text);
                        });
                    }
                    Line::Agent(text) => {
                        ui.horizontal(|ui| {
                            ui.strong("Agent");
                        });
                        egui_commonmark::CommonMarkViewer::new().show(
                            ui,
                            &mut state.commonmark,
                            text,
                        );
                    }
                    Line::Thinking(thought) => {
                        egui::CollapsingHeader::new("Thinking…")
                            .id_salt(("thinking", thought.len()))
                            .default_open(false)
                            .show(ui, |ui| {
                                ui.weak(thought);
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
