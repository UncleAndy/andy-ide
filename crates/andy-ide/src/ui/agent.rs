//! The conversation panel: who is talking, what was said, and the question on screen.

use std::time::Duration;

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
    status(ui, state);
}

/// The line under the conversation: how long the turn takes, which model answers, how
/// hard it is thinking, and how much of its window is spent.
///
/// Everything here is read from the agent and never guessed: an agent that says nothing
/// about its context window gets a dash, not an invented number. The two choices are
/// menus rather than labels because a model or a level that cannot be changed is a fact
/// about the agent, and one that can is a decision about the answer being written.
fn status(ui: &mut Ui, state: &mut AppState) {
    // Wrapped, not one line: a narrow panel would push the model and the context figure
    // past its right edge, and a fact that is off-screen is a fact nobody has.
    ui.horizontal_wrapped(|ui| {
        match state.agent.elapsed() {
            Some(took) => {
                ui.label(format!("⏱ {:.1} s", took.as_secs_f64()));
            }
            None => {
                ui.weak("⏱ —");
            }
        }
        if state.agent.busy {
            // A running timer has to be repainted to move, and the panel is otherwise
            // idle while an answer is coming.
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        ui.separator();

        let model = state.agent.current_model.clone();
        if state.agent.models.is_empty() {
            ui.weak("model —");
        } else {
            let shown = model
                .as_deref()
                .and_then(|id| {
                    state
                        .agent
                        .models
                        .iter()
                        .find(|model| model.id == id)
                        .map(|model| model.name.clone())
                })
                .unwrap_or_else(|| "model —".to_owned());
            // A menu button rather than a combo box: the name of the model is then part of
            // the button's own text, so it is on screen and readable, not only inside a
            // popup that has to be opened to find out what is answering.
            let choices = state.agent.models.clone();
            let mut asked = None;
            ui.menu_button(format!("{shown} ▾"), |ui| {
                for option in &choices {
                    let chosen = model.as_deref() == Some(option.id.as_str());
                    if ui.selectable_label(chosen, &option.name).clicked() && !chosen {
                        asked = Some(option.id.clone());
                        ui.close();
                    }
                }
            });
            if let Some(asked) = asked {
                state.agent.set_model(&asked);
            }
        }
        ui.separator();

        let mode = state.agent.current_mode.clone();
        if state.agent.modes.is_empty() {
            ui.weak("thinking —");
        } else {
            let shown = mode
                .as_deref()
                .and_then(|id| {
                    state
                        .agent
                        .modes
                        .iter()
                        .find(|mode| mode.id == id)
                        .map(|mode| mode.name.clone())
                })
                .unwrap_or_else(|| "thinking —".to_owned());
            let choices = state.agent.modes.clone();
            let mut asked = None;
            ui.menu_button(format!("{shown} ▾"), |ui| {
                for option in &choices {
                    let chosen = mode.as_deref() == Some(option.id.as_str());
                    if ui.selectable_label(chosen, &option.name).clicked() && !chosen {
                        asked = Some(option.id.clone());
                        ui.close();
                    }
                }
            });
            if let Some(asked) = asked {
                state.agent.set_mode(&asked);
            }
        }
        ui.separator();

        match state.agent.usage {
            Some(usage) => {
                let text = match usage.window {
                    // A percentage of nothing is not a number; without a window only the
                    // count is shown.
                    Some(window) if window > 0 => format!(
                        "ctx {}% of {}",
                        (usage.used.saturating_mul(100) / window).min(100),
                        short_tokens(window)
                    ),
                    _ => format!("ctx {} tokens", short_tokens(usage.used)),
                };
                ui.label(text);
            }
            None => {
                ui.weak("ctx —");
            }
        }
    });
}

/// Tokens, small enough to read at a glance: `4200`, `42k`, `1.2M`.
fn short_tokens(tokens: u64) -> String {
    if tokens < 10_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{}k", tokens / 1000)
    } else {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    }
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
        // The field stays open while the agent answers: a message typed mid-answer is
        // queued, and the keyboard stays where the person is writing. It is closed only
        // when there is no agent to talk to.
        let busy = !state.agent.running();
        // The field stops short of the button: a field that takes the whole width pushes
        // the button out of the panel, and a button nobody can see is a button nobody
        // can press.
        let width = (ui.available_width() - 90.0).max(80.0);
        // A name of its own, so the keyboard can be asked back for it after a send.
        let id = egui::Id::new("agent-message");
        let response = ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut state.agent.input)
                .id(id)
                .hint_text("Ask the agent…")
                .desired_width(width),
        );
        // Enter in the field sends: the field has the keyboard, so Enter cannot mean
        // anything else here, and a single-line field gives the focus up as it commits.
        let sent = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if sent {
            state.agent.send();
        }
        let pressed = ui.add_enabled(!busy, egui::Button::new("Send")).clicked();
        if pressed {
            state.agent.send();
        }
        // Sending is not leaving the field. Enter gives the focus up as it commits, and a
        // click puts it on the button; either way the editor is the next thing that finds
        // the keyboard free and takes it, and the next message is typed into a file.
        if sent || pressed {
            state.agent.wants_focus = true;
        }
        // Asked for on every frame until it is held, not once: a request is not the
        // keyboard, and a panel drawn later in the same frame asks louder.
        if state.agent.wants_focus {
            if response.has_focus() {
                state.agent.wants_focus = false;
            } else {
                response.request_focus();
                ui.ctx().request_repaint();
            }
        }
        // A click anywhere else means the person has left the field, and is obeyed at
        // once: the field does not hold the keyboard against a click.
        if ui.input(|input| input.pointer.any_click()) && !response.has_focus() {
            state.agent.wants_focus = false;
        }
    });
}
