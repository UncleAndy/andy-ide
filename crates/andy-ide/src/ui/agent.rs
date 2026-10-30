//! The conversation panel: who is talking, what was said, and the question on screen.

use std::time::Duration;

use egui::Ui;

use crate::agent::Line;
use crate::state::AppState;
use ide_agents::{AgentCommand, Model};

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
    // A start in flight repaints itself, or its "\u{2026}" would sit still and the panel
    // would look stuck at a step it had finished a moment before.
    if state.agent.connecting {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    header(ui, state);
    // The card the agent is waiting on is drawn above the conversation, before it: drawn
    // below, it would have to fit into what the conversation left over, and a card taller
    // than that — a question with long variants — would be pushed past the panel's own
    // edge, where it is clipped and takes no click. Here it takes its own height and the
    // conversation keeps what is left.
    state.agent.card_rect = None;
    question(ui, state);
    choice(ui, state);
    ui.separator();
    transcript(ui, state);
    ui.separator();
    // The field, the facts and the button are one foot, drawn together by `input`: the height
    // of the foot is the height of the field and one row, and nothing else may change it.
    input(ui, state);
    // The list of models floats in a layer of its own, above the button that opened it.
    models(ui, state);
    // What the rows under the conversation take, for the conversation to leave next frame: the
    // field and the single row under it, and nothing else. Measured against the bottom of the
    // panel, the reserve also carried the panel's own padding, and it was measured against a
    // drawing the conversation itself had moved — two numbers each leaning on the other.
    if let Some(field) = state.agent.field_rect {
        let row = ui.spacing().interact_size.y;
        state.agent.below = (field.height() + row + 6.0).max(24.0);
    }
}

/// The colours the panel asks a person in: amber for a permission, blue for a choice among
/// variants. Nothing else in the panel uses either, so the colour alone says who is waiting.
const ASKING: egui::Color32 = egui::Color32::from_rgb(226, 192, 96);
const CHOOSING: egui::Color32 = egui::Color32::from_rgb(140, 190, 220);

/// The card a question is drawn in, and its edge: a shade above the panel's own background,
/// so the place a person has to act in is visible before the words are read.
const CARD: egui::Color32 = egui::Color32::from_rgb(26, 29, 34);
const CARD_EDGE: egui::Color32 = egui::Color32::from_rgb(54, 60, 70);

/// Whether the key was pressed with nothing else held.
///
/// egui's `consume_key` asks for the modifiers a shortcut *needs* and lets others be held
/// alongside, so `Shift`+`Enter` would answer to a plain `Enter` — and send a message, or
/// answer a question, instead of making the new line it is meant to make.
fn plain_key(ui: &mut Ui, key: egui::Key) -> bool {
    let nothing_held = ui.input(|input| input.modifiers == egui::Modifiers::NONE);
    nothing_held && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key))
}

/// A line of text as heavy as a bold face would be.
///
/// egui's own `strong()` brightens the colour and nothing else: the fonts it ships have no
/// bold face, and asking for one gets the same light one. So the line is drawn twice, a
/// fraction of a point apart — which is what a heavier weight is, on the screen, when there
/// is no heavier weight to be had. The words are announced to the readable tree as they are:
/// nothing else knows the line is there.
fn bold_label(ui: &mut Ui, text: &str) {
    /// How far the second pass is shifted: enough to thicken, not enough to smear.
    const THICKEN: f32 = 0.6;

    let font = egui::TextStyle::Body.resolve(ui.style());
    let colour = ui.visuals().strong_text_color();
    let galley = ui
        .painter()
        .layout(text.to_owned(), font, colour, ui.available_width());
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), galley.size().y),
        egui::Sense::hover(),
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), text));
    ui.painter().galley(rect.left_top(), galley.clone(), colour);
    ui.painter()
        .galley(rect.left_top() + egui::vec2(THICKEN, 0.0), galley, colour);
}

/// What a request of the person's own is drawn in: a shade above the panel, square corners,
/// so that the question and the answer are two things and not one column of words.
const REQUEST: egui::Color32 = egui::Color32::from_rgb(34, 38, 45);
const REQUEST_EDGE: egui::Color32 = egui::Color32::from_rgb(58, 64, 74);

/// The card's own measurements, in points: the padding inside its frame, its vertical inner
/// margin and its outline, the gap under its heading, the room its refusal row takes, and one
/// answer on one line with the gap under it.
///
/// The answers' window is worked out from what the heading actually took, not from an
/// estimate of it: the heading is a sentence of the agent's and it wraps to two lines as
/// easily as one, and an estimate that comes up short is a card taller than the panel.
const CARD_PAD: f32 = 10.0;
const CARD_MARGIN: f32 = 8.0;
const CARD_OUTLINE: f32 = 1.0;
const CARD_GAP: f32 = 6.0;
const CARD_ROW: f32 = 29.0;
const CARD_REFUSAL: f32 = 26.0;
/// Kept back beyond the arithmetic above, because the scroll area has a margin of its own and
/// being a little short of room is a card that keeps a row back — never a card off the panel.
const CARD_SLACK: f32 = 8.0;

/// The most of the panel's height a card may take. It is not all of it: a question that
/// covers the whole panel takes the conversation with it, and the conversation is what a
/// person reads to decide which answer is right.
const CARD_SHARE: f32 = 0.75;

/// One answer on offer: what goes back to the agent, what to show, and what to say about it.
struct Offer {
    /// The word the agent is sent.
    value: String,
    /// What the person reads.
    title: String,
    /// What the pointer says when it rests on the answer.
    hint: String,
}

/// The panel's own rect: what the panel is drawn as, which is what a card has to fit in.
///
/// `shown_rect` is written by the panel as it draws, and it is the drawn rect. egui's own
/// idea of the available space is larger in a dock, and a card that trusts it is drawn past
/// the panel's edge.
fn room_of(state: &AppState, ui: &Ui) -> egui::Rect {
    state.agent.shown_rect.unwrap_or_else(|| ui.clip_rect())
}

/// What a card says: the question, the answers, whose colour it is drawn in, and the way out.
struct Card<'a> {
    accent: egui::Color32,
    heading: &'a str,
    offers: &'a [Offer],
    /// Whether several answers may be chosen at once.
    multiple: bool,
    /// Whether the answer is written rather than picked.
    free_text: bool,
    /// The label of the way out of a question whose answers a person wants none of.
    refusal: &'a str,
    /// What the pointer says when it rests on the way out.
    refusal_hint: &'a str,
}

/// The agent waiting for a person, drawn as one card.
///
/// Both kinds of waiting look the same — a permission and a choice among variants are the
/// same event from the panel's side: the agent has stopped and only a person can start it
/// again. Two shapes for one situation would make a person learn both.
///
/// `room` is the panel's own rect — what is on screen — and not what egui offers as the
/// available space. In a dock the two are not the same: the available width is the room the
/// panel *could* be given, which is wider than the panel is drawn, and a card laid out to it
/// comes out wider than the panel, taking the refusal, which is right aligned, off the edge
/// with it.
///
/// Returns the answer when one was given — `Some(Some(values))` for the answers picked,
/// `Some(None)` for a refusal — and the rect the card was drawn in.
fn waiting_card(
    ui: &mut Ui,
    room: egui::Rect,
    card: &Card<'_>,
    picked: &mut Vec<String>,
    typed: &mut String,
    wants_focus: &mut bool,
) -> (Option<Option<Vec<String>>>, egui::Rect) {
    // Two points are taken off as well as the padding: the frame's own outline.
    let width = (room.width() - 2.0 * CARD_PAD - 2.0).max(80.0);
    // How much of the panel the card may take: a share of it, so that the conversation stays
    // visible — but never more than is left under the rows drawn before the card, and never so
    // little that a couple of answers do not show.
    let floor = CARD_REFUSAL + CARD_GAP + 2.0 * (CARD_MARGIN + CARD_OUTLINE) + 2.0 * CARD_ROW;
    let allowed = (room.height() * CARD_SHARE).max(floor);
    let window = allowed.min(ui.available_height().max(floor));

    let mut chosen = None;
    let frame = egui::Frame::group(ui.style())
        .fill(CARD)
        .stroke(egui::Stroke::new(CARD_OUTLINE, CARD_EDGE))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(CARD_PAD as i8, CARD_MARGIN as i8))
        .show(ui, |ui| {
            // The card is as wide as the panel: a question that stops halfway across leaves
            // the eye to work out where it ends.
            ui.set_width(width);
            // Wrapped, not on one line: the agent's question is a sentence, and it has to
            // fit inside the card rather than run out of the panel's edge.
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("\u{25c6}").color(card.accent).small());
                ui.label(
                    egui::RichText::new(card.heading)
                        .color(card.accent)
                        .strong(),
                );
            });
            ui.add_space(CARD_GAP);
            // What is left for the answers, from what the heading took: the refusal row and
            // the frame's own margins are known, the heading is not.
            let taken = ui.min_rect().height() + CARD_GAP;
            let answers = (window
                - taken
                - CARD_REFUSAL
                - CARD_GAP
                - 2.0 * (CARD_MARGIN + CARD_OUTLINE)
                - CARD_SLACK)
                .max(CARD_ROW);
            // The answers scroll in a window of their own; the heading and the refusal stay
            // where they are, because the refusal is the way out of a question a person
            // wants none of the answers to, and the way out has to be on screen.
            //
            // The scroll area is named. An unnamed one takes its id from the ui it is drawn
            // in, and the conversation's scroll area is drawn in the same ui: two of them
            // under one id share their state and egui, in a debug build, draws a red box and
            // a `\u{1f525} ID clash` over both.
            if card.free_text {
                // Nothing to choose between: the answer is written, and `Enter` answers the
                // question the way it sends a message in the field under the conversation.
                let here = egui::Id::new("agent-answer");
                // The key is read before the field is drawn: a field of one line hands the focus
                // back on `Enter`, and by the time it has drawn itself the key is spent.
                let answered =
                    ui.memory(|memory| memory.has_focus(here)) && plain_key(ui, egui::Key::Enter);
                let field = ui.add(
                    egui::TextEdit::singleline(typed)
                        .id(here)
                        .hint_text("Write the answer\u{2026}")
                        .desired_width(width),
                );
                if *wants_focus {
                    if field.has_focus() {
                        *wants_focus = false;
                    } else {
                        field.request_focus();
                        ui.ctx().request_repaint();
                    }
                }
                let written = typed.trim().to_owned();
                let pressed = ui
                    .add_enabled(!written.is_empty(), egui::Button::new("Answer"))
                    .clicked();
                if !written.is_empty() && (pressed || answered) {
                    chosen = Some(Some(vec![written]));
                }
                ui.add_space(2.0);
            }
            if !card.free_text {
                egui::ScrollArea::vertical()
                    .id_salt("agent-question")
                    .max_height(answers)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(width);
                        for offer in card.offers {
                            // One line each, whatever the answer's length: the reasons for three
                            // answers and for thirty differ by nothing else in the panel, and the
                            // whole text is a pointer's rest away. Answers that wrap make a card
                            // as tall as the panel for no answer's sake.
                            // When several may be chosen, an answer is a switch rather than a
                            // button: pressing one leaves the question open, because the answer
                            // is the set of them.
                            let answer = if card.multiple {
                                let picked_already =
                                    picked.iter().any(|value| value == &offer.value);
                                egui::Button::selectable(picked_already, &offer.title)
                            } else {
                                egui::Button::new(&offer.title)
                            };
                            let answer = answer
                                .min_size(egui::vec2(width, 26.0))
                                .corner_radius(5)
                                .wrap_mode(egui::TextWrapMode::Truncate);
                            if ui.add(answer).on_hover_text(&offer.hint).clicked() {
                                if card.multiple {
                                    if let Some(at) =
                                        picked.iter().position(|value| value == &offer.value)
                                    {
                                        picked.remove(at);
                                    } else {
                                        picked.push(offer.value.clone());
                                    }
                                } else {
                                    chosen = Some(Some(vec![offer.value.clone()]));
                                }
                            }
                            ui.add_space(3.0);
                        }
                    });
            }
            ui.add_space(2.0);
            // The row is given the height of the button and no more. `with_layout` is handed
            // the whole available size and starts its child at the top of it, so a
            // right-aligned row left to itself stretches the card to the bottom of the panel
            // — the card would be as tall as the panel whatever it held.
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), CARD_REFUSAL),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui
                        .button(egui::RichText::new(card.refusal).weak())
                        .on_hover_text(card.refusal_hint)
                        .clicked()
                    {
                        chosen = Some(None);
                    }
                    // Several answers are gathered first and sent together: one click cannot
                    // mean "these, and also those".
                    if card.multiple {
                        let ready = !picked.is_empty();
                        let hint = if ready {
                            format!("the agent is told: {}", picked.join(", "))
                        } else {
                            "pick at least one answer first".to_owned()
                        };
                        let send = ui
                            .add_enabled(ready, egui::Button::new("Answer"))
                            .on_hover_text(hint);
                        if send.clicked() {
                            chosen = Some(Some(picked.clone()));
                        }
                    }
                },
            );
        });
    (chosen, frame.response.rect)
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
                option.title.clone()
            } else {
                format!("{}\nthe agent is told: {}", option.title, option.value)
            },
        })
        .collect();
    let room = room_of(state, ui);
    let multiple = request.multiple;
    let (answer, drawn) = waiting_card(
        ui,
        room,
        &Card {
            accent: CHOOSING,
            heading: &heading,
            offers: &offers,
            multiple,
            free_text: request.free_text,
            refusal: "None of these",
            refusal_hint: "the agent is told you wanted none of them",
        },
        &mut state.agent.picked,
        &mut state.agent.card_text,
        &mut state.agent.card_wants_focus,
    );
    state.agent.card_rect = Some(drawn);
    if let Some(answer) = answer {
        state.agent.answer_choice(answer);
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
    // One row, always. A wrapped row is a second row, and the second row is the height of the
    // panel's foot, which belongs to the field alone — and a row that wraps is a row that grows
    // by itself: the moment a figure in it gains a character, the timer does that several times
    // a second, the row comes out a line taller, and the panel is repainted only when something
    // moves. That is the foot growing while the mouse is moved. What does not fit is left out,
    // with an ellipsis saying so: a fact that is off the edge and a fact not drawn are the same
    // fact, and only one of them costs the conversation a line.
    let mut dropped = false;
    let row = ui.horizontal(|ui| {
        'facts: {
            let elapsed = match state.agent.elapsed() {
                Some(took) => format!("⏱ {:.1} s", took.as_secs_f64()),
                None => "⏱ —".to_owned(),
            };
            if !room_for(ui, &elapsed) {
                dropped = true;
                break 'facts;
            }
            ui.label(elapsed);
            if state.agent.busy {
                // A running timer has to be repainted to move, and the panel is otherwise
                // idle while an answer is coming.
                ui.ctx().request_repaint_after(Duration::from_millis(200));
            }
            ui.separator();

            let model = state.agent.current_model.clone();
            if !room_for(ui, "model — a model name") {
                dropped = true;
                break 'facts;
            }
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
            if !room_for(ui, "thinking — a level") {
                dropped = true;
                break 'facts;
            }
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
            if !room_for(ui, "mode — a mode name") {
                dropped = true;
                break 'facts;
            }
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

            let usage_text = match &state.agent.usage {
                Some(usage) => match usage.window {
                    // A percentage of nothing is not a number; without a window only the count is
                    // shown.
                    Some(window) if window > 0 => format!(
                        "ctx {}% of {}",
                        (usage.used.saturating_mul(100) / window).min(100),
                        short_tokens(window)
                    ),
                    _ => format!("ctx {} tokens", short_tokens(usage.used)),
                },
                None => "ctx —".to_owned(),
            };
            if !room_for(ui, &usage_text) {
                dropped = true;
                break 'facts;
            }
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
            if dropped {
                // The row is one row: what was left out is said by a mark in it, never by a
                // second row under it.
                ui.weak("\u{2026}");
            }
        }
    });
    state.agent.status_rect = Some(row.response.rect);
}

/// Whether a fact of this shape has room in the row that is left, measured the way egui will
/// draw it, with a margin for the padding a button adds around its text.
fn room_for(ui: &Ui, text: &str) -> bool {
    let height = ui.text_style_height(&egui::TextStyle::Body);
    let width = ui
        .painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(height),
            egui::Color32::PLACEHOLDER,
        )
        .size()
        .x;
    // The padding of a button, the separator that follows it, and the ellipsis that may have to
    // come after everything: a fact that fits exactly is a fact that pushes the mark off.
    ui.available_width() >= width + 46.0
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
        if state.agent.connecting {
            // The steps of a start, and a mark that moves: the step says what is happening,
            // and the moving mark is the proof that it still is. A line that sits still
            // while a real agent takes its time to answer looks like a hang.
            let phase = state.agent.connecting_step();
            let dots = match (ui.input(|input| input.time) * 2.0) as usize % 3 {
                0 => "\u{2026}",
                1 => "..",
                _ => ".",
            };
            ui.weak(format!("{phase}{dots}"));
        } else {
            ui.weak(&state.agent.status);
        }
    });
    // The row of the name and the command is the desk the agent is set up at; a running
    // agent is not being set up — the fields are read-only then anyway — and the strip is
    // only a row the conversation did not ask for. It comes back when the agent stops,
    // because that is when the next setup happens. There is no separate field for the
    // terminal: the terminal belongs to the agent, and the editor only has to say where
    // the project is and that files are worked on through it.
    let running = state.agent.running();
    if running {
        return;
    }
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
    // What the rows below the conversation took last frame, and a line's worth more: the field
    // grows a line at a time, and the room for that line has to be given up on the frame it is
    // typed, not the one after it.
    let below = state.agent.below + ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let height = (ui.available_height() - below).max(60.0);
    egui::ScrollArea::vertical()
        .id_salt("agent-transcript")
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .max_height(height)
        .show(ui, |ui| {
            for (index, line) in lines.iter().enumerate() {
                match line {
                    Line::You(text) => {
                        // The request is a block of its own, square and set in bold: an answer
                        // is read against the question, and the question has to be findable by
                        // looking rather than by reading.
                        egui::Frame::new()
                            .fill(REQUEST)
                            .stroke(egui::Stroke::new(1.0, REQUEST_EDGE))
                            .corner_radius(0)
                            .inner_margin(egui::Margin::symmetric(8, 5))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                bold_label(ui, text);
                            });
                        ui.add_space(2.0);
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
                        // The id is the block's place in the conversation, not the length
                        // of what was thought: a restored conversation holds thoughts of
                        // equal length, and a thought being written changes length with
                        // every chunk — two blocks on one id is what egui draws its warning
                        // box over (red frames over the transcript after a session is
                        // restored, and a box that flickers while an answer streams).
                        egui::CollapsingHeader::new("Thinking…")
                            .id_salt(("thinking", index))
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
    let room = room_of(state, ui);
    let mut none = Vec::new();
    let mut nothing_written = String::new();
    let mut no_focus = false;
    let (answer, drawn) = waiting_card(
        ui,
        room,
        &Card {
            accent: ASKING,
            heading: &heading,
            offers: &offers,
            multiple: false,
            free_text: false,
            refusal: "Refuse",
            refusal_hint: "the agent is told no",
        },
        &mut none,
        &mut nothing_written,
        &mut no_focus,
    );
    state.agent.card_rect = Some(drawn);
    // A permission is answered with one option or with none: the card hands back a list for
    // the shape of a question that allows several, and this one never does.
    if let Some(answer) = answer {
        state
            .agent
            .answer(answer.and_then(|values| values.into_iter().next()));
    }
}

/// The field, and the button that sends it.
fn input(ui: &mut Ui, state: &mut AppState) {
    /// One command row in the list: the height of a row, and how many are shown at most.
    const ROW: f32 = 20.0;
    const ROWS_SHOWN: usize = 8;

    let busy = !state.agent.running();
    // A name of its own, so the keyboard can be asked back for it after a send.
    let id = egui::Id::new("agent-message");
    let has_keyboard = ui.memory(|memory| memory.has_focus(id));

    // What is typed so far, if it is a command being typed: a slash, then the name, and
    // nothing else. A space means the argument has begun, and then there is nothing to offer.
    let typed = state
        .agent
        .input
        .strip_prefix('/')
        .filter(|rest| !rest.contains(' '));
    // Read out of the panel and owned here: the list is drawn after the field, and the field
    // is where the person is typing — a borrow of the commands would be in the way of it.
    let matches: Vec<AgentCommand> = match typed {
        Some(typed) => state
            .agent
            .commands
            .iter()
            .filter(|command| {
                command
                    .name
                    .to_lowercase()
                    .starts_with(&typed.to_lowercase())
            })
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    if state.agent.command_pick >= matches.len() {
        state.agent.command_pick = 0;
    }

    // The keyboard is read before the field is drawn: in a field of several lines `Enter`
    // would otherwise go to a new line, and the key is spent by the time the field has drawn
    // itself. `Shift`+`Enter` is left alone, and that is what makes a new line.
    if has_keyboard && !matches.is_empty() {
        if plain_key(ui, egui::Key::ArrowDown) {
            state.agent.command_pick = (state.agent.command_pick + 1) % matches.len();
        }
        if plain_key(ui, egui::Key::ArrowUp) {
            state.agent.command_pick =
                (state.agent.command_pick + matches.len() - 1) % matches.len();
        }
        let complete = plain_key(ui, egui::Key::Enter) | plain_key(ui, egui::Key::Tab);
        if complete {
            state.agent.input = format!("/{} ", matches[state.agent.command_pick].name);
            state.agent.wants_focus = true;
            return;
        }
    }
    // Enter sends. Without the keyboard the field is not the one being typed in, and the key
    // belongs to whoever holds it.
    let enter = has_keyboard && plain_key(ui, egui::Key::Enter);

    // The field takes the width it can and three lines to start with: it is where a message of
    // several lines is written, and it grows by itself as the lines are added.
    let field = egui::TextEdit::multiline(&mut state.agent.input)
        .id(id)
        .hint_text("Ask the agent\u{2026}")
        .desired_rows(3)
        .desired_width(f32::INFINITY);
    let response = ui.add_enabled(!busy, field);
    state.agent.field_rect = Some(response.rect);

    // The foot of the panel: one row under the field, the facts about the agent on the left and
    // the button that sends on the right. The height of the foot is then the height of the field
    // plus one row of a fixed height, and the conversation above it cannot change it.
    const SEND_ROOM: f32 = 88.0;
    let row_height = ui.spacing().interact_size.y;
    // An answer being written is what makes the foot's button a different button: the
    // person is either sending the next message or finishing with this one.
    let answering = state.agent.busy;
    let mut pressed = false;
    let mut cancels = false;
    ui.horizontal(|ui| {
        let facts_room = (ui.available_width() - SEND_ROOM).max(0.0);
        ui.allocate_ui_with_layout(
            egui::vec2(facts_room, row_height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| status(ui, state),
        );
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), row_height),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if answering {
                    // Stopping the answer is not stopping the agent. The turn is asked to
                    // end (ACP `session/cancel`) and the process stays where it is — the
                    // button is the answer's door, not the session's.
                    if ui
                        .button("Cancel")
                        .on_hover_text("Stop this answer; the agent stays running")
                        .clicked()
                    {
                        cancels = true;
                    }
                } else if ui.add_enabled(!busy, egui::Button::new("Send")).clicked() {
                    pressed = true;
                }
            },
        );
    });

    if cancels {
        state.agent.cancel();
    }
    if enter || pressed {
        state.agent.send();
        // Sending is not leaving the field: the next message is typed here, not into a file.
        state.agent.wants_focus = true;
    }
    // Asked for on every frame until it is held, not once: a request is not the keyboard, and
    // a panel drawn later in the same frame asks louder.
    if state.agent.wants_focus {
        if response.has_focus() {
            state.agent.wants_focus = false;
        } else {
            response.request_focus();
            ui.ctx().request_repaint();
        }
    }
    // A click anywhere else means the person has left the field, and is obeyed at once: the
    // field does not hold the keyboard against a click.
    if ui.input(|input| input.pointer.any_click()) && !response.has_focus() {
        state.agent.wants_focus = false;
    }

    // The commands, over the field they are typed into, while they are being typed.
    if matches.is_empty() {
        return;
    }
    let rows = matches.len().min(ROWS_SHOWN);
    let height = rows as f32 * ROW + 8.0;
    let mut completed = None;
    egui::Area::new(egui::Id::new("agent-commands"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(response.rect.left(), response.rect.top() - 4.0))
        .default_size(egui::vec2(360.0, height))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(320.0);
                for (index, command) in matches.iter().enumerate().take(ROWS_SHOWN) {
                    let row = egui::Button::selectable(
                        index == state.agent.command_pick,
                        format!("/{}  {}", command.name, command.description),
                    )
                    .min_size(egui::vec2(ui.available_width(), ROW))
                    .corner_radius(2);
                    let hint = match &command.hint {
                        Some(hint) => format!("/{} \u{2014} takes {}", command.name, hint),
                        None => format!("/{} takes nothing", command.name),
                    };
                    if ui.add(row).on_hover_text(hint).clicked() {
                        completed = Some(command.name.clone());
                    }
                }
            });
        });
    if let Some(name) = completed {
        state.agent.input = format!("/{name} ");
        state.agent.wants_focus = true;
    }
}

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
