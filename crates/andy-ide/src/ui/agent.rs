//! The conversation panel: who is talking, what was said, and the question on screen.

use std::time::Duration;

use egui::Ui;

use crate::agent::Line;
use crate::state::AppState;
use ide_agents::Model;

/// The panel.
pub fn panel(ui: &mut Ui, state: &mut AppState) {
    // Where the panel was drawn, for the tests that point at it.
    state.agent.shown_rect = Some(ui.max_rect());
    state.agent.pump();
    // The copy shortcut was taken before the editor drew, so that it did not copy a line
    // of code instead; here it becomes the event egui's text selection listens for, while
    // the words are being drawn.
    if state.agent.wants_copy {
        state.agent.wants_copy = false;
        ui.ctx()
            .input_mut(|input| input.events.push(egui::Event::Copy));
    }
    // A restart that is due has to be looked at again: the panel is otherwise idle
    // between frames, and a timer nobody repaints for never fires.
    if state.agent.waiting_to_reconnect() {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }
    header(ui, state);
    // The card the agent is waiting on is drawn above the conversation, before it: drawn
    // below, it would have to fit into what the conversation left over, and a card taller
    // than that — a question with long variants — would be pushed past the panel's own
    // edge, where it is clipped and takes no click. Here it takes its own height and the
    // conversation keeps what is left.
    question(ui, state);
    choice(ui, state);
    ui.separator();
    transcript(ui, state);
    ui.separator();
    input(ui, state);
    status(ui, state);
    // The list of models floats in a layer of its own, above the button that opened it.
    models(ui, state);
}

/// The colours the panel asks a person in: amber for a permission, blue for a choice among
/// variants. Nothing else in the panel uses either, so the colour alone says who is waiting.
const ASKING: egui::Color32 = egui::Color32::from_rgb(226, 192, 96);
const CHOOSING: egui::Color32 = egui::Color32::from_rgb(140, 190, 220);

/// The card a question is drawn in, and its edge: a shade above the panel's own background,
/// so the place a person has to act in is visible before the words are read.
const CARD: egui::Color32 = egui::Color32::from_rgb(26, 29, 34);
const CARD_EDGE: egui::Color32 = egui::Color32::from_rgb(54, 60, 70);

/// One answer on offer: what goes back to the agent, what to show, and what to say about it.
struct Offer {
    /// The word the agent is sent.
    value: String,
    /// What the person reads.
    title: String,
    /// What the pointer says when it rests on the answer.
    hint: String,
}

/// The agent waiting for a person, drawn as one card.
///
/// Both kinds of waiting look the same — a permission and a choice among variants are the
/// same event from the panel's side: the agent has stopped and only a person can start it
/// again. Two shapes for one situation would make a person learn both.
///
/// Returns the answer when one was given: `Some(Some(value))` for the answer picked,
/// `Some(None)` for a refusal.
fn waiting_card(
    ui: &mut Ui,
    accent: egui::Color32,
    heading: &str,
    offers: &[Offer],
    refusal: &str,
    refusal_hint: &str,
) -> Option<Option<String>> {
    let mut chosen = None;
    egui::Frame::group(ui.style())
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, CARD_EDGE))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            // The card is as wide as the panel: a question that stops halfway across leaves
            // the eye to work out where it ends.
            ui.set_width(ui.available_width());
            // Wrapped, not on one line: the agent's question is a sentence, and it has to
            // fit inside the card rather than run out of the panel's edge.
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("\u{25c6}").color(accent).small());
                ui.label(egui::RichText::new(heading).color(accent).strong());
            });
            ui.add_space(6.0);
            let width = ui.available_width();
            for offer in offers {
                let answer = egui::Button::new(&offer.title)
                    .min_size(egui::vec2(width, 26.0))
                    .corner_radius(5)
                    .wrap_mode(egui::TextWrapMode::Wrap);
                if ui.add(answer).on_hover_text(&offer.hint).clicked() {
                    chosen = Some(Some(offer.value.clone()));
                }
                ui.add_space(3.0);
            }
            ui.add_space(2.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(egui::RichText::new(refusal).weak())
                    .on_hover_text(refusal_hint)
                    .clicked()
                {
                    chosen = Some(None);
                }
            });
        });
    chosen
}

/// The variants the agent offers, when it is waiting for one to be picked.
///
/// ACP calls this an elicitation: the agent cannot decide which way to go and asks the
/// person. What goes back is the variant's *value*; the title is for the person, and the
/// two are often not the same word.
fn choice(ui: &mut Ui, state: &mut AppState) {
    let Some((_, request)) = &state.agent.choice else {
        return;
    };
    let heading = request.title().to_owned();
    let offers: Vec<Offer> = request
        .options
        .iter()
        .map(|option| Offer {
            value: option.value.clone(),
            title: option.title.clone(),
            hint: if option.value == option.title {
                "the agent is told this is the one".to_owned()
            } else {
                format!("the agent is told: {}", option.value)
            },
        })
        .collect();
    if let Some(answer) = waiting_card(
        ui,
        CHOOSING,
        &heading,
        &offers,
        "None of these",
        "the agent is told you wanted none of them",
    ) {
        state.agent.answer_choice(answer.as_deref());
    }
}

/// The models on offer, folded by provider, in a layer above the button that opened it.
///
/// It is its own area rather than a menu popup: a popup is laid out inside the rect it
/// measured on an earlier frame, and a scroll area that shrinks to that rect clips its own
/// last row — an entry that is on screen and yet takes no click. An area whose rectangle is
/// its content's own grows upward from the button and is drawn in full.
///
/// The rectangle is capped by `limit` and the rest scrolls, which is the whole point: an
/// agent wired to several providers offers more models than a panel can show.
fn models(ui: &mut Ui, state: &mut AppState) {
    let Some(anchor) = state.agent.model_button else {
        return;
    };
    if !state.agent.models_open || state.agent.models.is_empty() {
        return;
    }
    let choices = crate::agent::grouped(&state.agent.models);
    let mut asked = None;
    let mut chosen = false;
    // Never more than part of the window: a list that grew to fill it would cover the
    // conversation it belongs to.
    let limit = (ui.ctx().content_rect().height() * 0.4).max(120.0);
    // The box is given its height instead of asking for one. A scroll area reports the
    // height it was given, so a layer that sized itself from its content would shrink by
    // a row on every frame until its last entry fell outside — visible, and unclickable.
    // Counting the rows once settles it: the box is as tall as the rows are, up to the
    // cap, and only what is past the cap scrolls.
    let rows = choices
        .iter()
        .map(|(provider, group)| group.len() + usize::from(provider.is_some()))
        .sum::<usize>();
    let row = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
    let margins = ui.spacing().menu_margin.sum().y;
    let height = (rows as f32 * row + margins).min(limit);
    let area = egui::Area::new(egui::Id::new("agent-model-list"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(anchor.left(), anchor.top() - 4.0))
        .default_size(egui::vec2(240.0, height))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(220.0);
                ui.set_min_height(height);
                egui::ScrollArea::vertical()
                    .id_salt("model-list")
                    .max_height(height)
                    .show(ui, |ui| {
                        for (provider, group) in &choices {
                            if let Some(provider) = provider {
                                egui::CollapsingHeader::new(provider)
                                    .id_salt(("provider", provider))
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        for option in group {
                                            if pick(
                                                ui,
                                                option,
                                                state.agent.current_model.as_deref(),
                                                &mut asked,
                                            ) {
                                                chosen = true;
                                            }
                                        }
                                    });
                            } else {
                                for option in group {
                                    if pick(
                                        ui,
                                        option,
                                        state.agent.current_model.as_deref(),
                                        &mut asked,
                                    ) {
                                        chosen = true;
                                    }
                                }
                            }
                        }
                    });
            });
        });
    // Choosing ends the visit, and so does a click anywhere else or Escape — the way a
    // menu behaves, without a menu's sizing.
    // The click that opened the list is on the button, not "somewhere else": without
    // saying so, the list would shut on the very frame it opened.
    let clicked_outside = ui.input(|input| input.pointer.any_click())
        && ui
            .ctx()
            .pointer_interact_pos()
            .is_some_and(|pos| !area.response.rect.contains(pos) && !anchor.contains(pos));
    if chosen || clicked_outside || ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        state.agent.models_open = false;
    }
    if let Some(asked) = asked {
        state.agent.set_model(&asked);
    }
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
            // A button rather than a combo box: the name of the model is then part of the
            // button's own text, so it is on screen and readable, not only inside a list
            // that has to be opened to find out what is answering. The list itself is
            // drawn by `models`, above this line.
            let button = ui
                .button(format!("{shown} ▾"))
                .on_hover_text("Choose the model that answers");
            state.agent.model_button = Some(button.rect);
            if button.clicked() {
                state.agent.models_open = !state.agent.models_open;
            }
        }
        ui.separator();

        // How hard the model thinks. The agent names the levels it has — ACP carries them
        // as a setting of its own, `thought_level`, and Hermes' are none, minimal, low,
        // medium, high, xhigh, max and ultra. An agent that offers no such setting gets a
        // button that says so rather than a menu of levels it would refuse: Hermes 0.22.5
        // over ACP offers only the edit policy, and there the level belongs to the model
        // in the Hermes profile.
        match crate::agent::thought_level(&state.agent.options) {
            Some(option) => {
                let config = option.id.clone();
                let current = option.current.clone();
                let choices = option.choices.clone();
                let shown = option.current_name().unwrap_or_else(|| option.name.clone());
                let mut asked = None;
                ui.menu_button(format!("{shown} ▾"), |ui| {
                    for choice in &choices {
                        let chosen = current.as_deref() == Some(choice.value.as_str());
                        if ui.selectable_label(chosen, &choice.name).clicked() && !chosen {
                            asked = Some(choice.value.clone());
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text(format!("{}: what the agent offers here", option.name));
                if let Some(asked) = asked {
                    state.agent.set_setting(&config, &asked);
                }
            }
            None => {
                ui.add_enabled(false, egui::Button::new("thinking —"))
                    .on_disabled_hover_text(
                        "this agent does not offer a level of thinking over ACP; Hermes sets it \
                         for a model in its own profile (agent.reasoning_overrides)",
                    );
            }
        }
        ui.separator();

        // What the agent may do without asking: ACP's modes, which are the edit policy and
        // never a level of thinking.
        let mode = state.agent.current_mode.clone();
        let permissions: Vec<_> = state.agent.modes.clone();
        if permissions.is_empty() {
            ui.weak("mode —");
        } else {
            let shown = mode
                .as_deref()
                .and_then(|id| {
                    permissions
                        .iter()
                        .find(|mode| mode.id == id)
                        .map(|mode| mode.name.clone())
                })
                .unwrap_or_else(|| "mode —".to_owned());
            let choices = permissions.clone();
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
                state.agent.name = spec.name.clone();
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
            .button("New session")
            .on_hover_text("Forget the conversation this project kept and start a fresh one")
            .clicked()
        {
            state.agent.new_session();
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
    // One row, the way the rest of the panel is: a name to call it by, and the command
    // that starts it. There is no separate field for the terminal, because the terminal
    // belongs to the agent — it is configured there, and the editor only has to say where
    // the project is and that files are worked on through it.
    let running = state.agent.running();
    ui.horizontal(|ui| {
        ui.label("Name");
        let name_width = (ui.available_width() * 0.3).max(60.0);
        ui.add_enabled(
            !running,
            egui::TextEdit::singleline(&mut state.agent.name)
                .hint_text("Hermes")
                .desired_width(name_width),
        )
        .on_hover_text("What the list calls this agent");

        ui.separator();
        ui.label("Command");
        let width = (ui.available_width() - 8.0).max(80.0);
        ui.add_enabled(
            !running,
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
    // What the rows below the conversation take: the separator, the field and the status
    // line. They are drawn after it, so the room is kept for them here.
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
    let heading = request.title().to_owned();
    let offers: Vec<Offer> = request
        .options
        .iter()
        .map(|option| Offer {
            value: option.option_id.clone(),
            title: option.name.clone(),
            // The id is what the agent hears, and it is the thing a person is agreeing to.
            hint: format!("the agent is told: {}", option.option_id),
        })
        .collect();
    if let Some(answer) = waiting_card(
        ui,
        ASKING,
        &heading,
        &offers,
        "Refuse",
        "the agent is told no",
    ) {
        state.agent.answer(answer);
    }
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

/// One model in the menu. True when it has just been chosen.
fn pick(ui: &mut Ui, option: &Model, current: Option<&str>, asked: &mut Option<String>) -> bool {
    let chosen = current == Some(option.id.as_str());
    // The list is grouped by provider, so the row says only the model: the provider is
    // on the group's own line above it.
    let label = crate::agent::listed_name(&option.name);
    if ui.selectable_label(chosen, label).clicked() && !chosen {
        *asked = Some(option.id.clone());
        return true;
    }
    false
}
