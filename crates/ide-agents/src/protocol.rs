//! The shapes this client reads out of an agent's own words.
//!
//! The protocol itself is spoken by the standard's own client (`crate::acp`); what is left here are the
//! readers that turn an agent's answer into the words this project uses — the models and modes a conversation
//! offers, the settings it can change, the pages of `session/list`, the questions an agent puts to the person
//! — and they are handed the same JSON they always were.
//!
//! Everything the protocol defines but this client does not act on is kept as `serde_json::Value` rather than
//! dropped: a newer agent sends fields this version has never heard of, and refusing a message because of them
//! would be a version of the protocol that only works with agents built the same week.

use serde::{Deserialize, Serialize};
use serde_json::Value;
/// One conversation the agent knows of, as `session/list` describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionInfo {
    /// The id to hand back to `session/load`.
    pub id: String,
    /// Where that conversation lives, if the agent says.
    pub cwd: Option<String>,
    /// What the agent calls it, if it names it at all.
    pub title: Option<String>,
    /// When it was last touched, in the agent's own words — a string, because agents differ and
    /// a date nobody can parse is still worth showing.
    pub updated_at: Option<String>,
}
/// Reads a page of `session/list`: the conversations, and where the next page starts.
pub fn sessions_of(value: &Value) -> (Vec<SessionInfo>, Option<String>) {
    let sessions = value
        .get("sessions")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    // `sessionId` is the name ACP gives it; a bare `id` is what an agent that
                    // reads the schema loosely may send.
                    let id = row
                        .get("sessionId")
                        .or_else(|| row.get("id"))
                        .and_then(Value::as_str)?
                        .to_owned();
                    Some(SessionInfo {
                        id,
                        cwd: row.get("cwd").and_then(Value::as_str).map(str::to_owned),
                        title: row.get("title").and_then(Value::as_str).map(str::to_owned),
                        updated_at: row
                            .get("updatedAt")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let next = value
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_owned);
    (sessions, next)
}
/// One model the agent offers, as `session/new` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    /// What to send back in `session/set_model`.
    pub id: String,
    /// What to show a person.
    pub name: String,
}
pub fn options_of(answer: &Value) -> Vec<ConfigOption> {
    answer
        .get("configOptions")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(option_of).collect())
        .unwrap_or_default()
}
/// One setting. A setting without an id is not one a client could change, so it is dropped.
///
/// A boolean setting — ACP allows them — carries no choices and no category this panel
/// knows; it is read as a setting with nothing to pick, which is what it is.
fn option_of(option: &Value) -> Option<ConfigOption> {
    let id = option.get("id").and_then(Value::as_str)?;
    Some(ConfigOption {
        id: id.to_owned(),
        name: option
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned(),
        category: option
            .get("category")
            .and_then(Value::as_str)
            .map(str::to_owned),
        current: option
            .get("currentValue")
            .and_then(Value::as_str)
            .map(str::to_owned),
        choices: option
            .get("options")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|choice| {
                        let value = choice.get("value").and_then(Value::as_str)?;
                        Some(ConfigChoice {
                            value: value.to_owned(),
                            name: choice
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(value)
                                .to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}
/// One mode the agent offers: in practice, how hard it should think.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    /// What to send back in `session/set_mode`.
    pub id: String,
    /// What to show a person.
    pub name: String,
}
/// The category ACP gives the setting a mode is carried in: Hermes reports an edit-policy
/// change as this setting and never as `current_mode_update`.
pub const MODE: &str = "mode";
/// The category ACP gives the setting for how hard a model thinks.
pub const THOUGHT_LEVEL: &str = "thought_level";
/// What Hermes calls that setting when it names one itself. A client goes by the category
/// and falls back to this id for an agent that sends none.
pub const REASONING: &str = "reasoning_effort";
/// One choice inside a setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigChoice {
    /// What goes back to the agent when this one is picked.
    pub value: String,
    /// What to call it.
    pub name: String,
}
/// A setting the agent offers to change: ACP's config option.
///
/// It carries what a person may pick for a session and what is neither the model nor the
/// mode — in Hermes, how hard the model thinks. The agent names the choices, so a panel
/// shows what it is given instead of a table of its own: a level the agent has never heard
/// of comes back as an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigOption {
    /// What to send back in `session/set_config_option`.
    pub id: String,
    /// What to call the setting.
    pub name: String,
    /// What kind of setting it is — `thought_level`, `mode`, `model` — when it says.
    pub category: Option<String>,
    /// The choice in force, by its value.
    pub current: Option<String>,
    /// The choices on offer, in the agent's own order.
    pub choices: Vec<ConfigChoice>,
}
impl ConfigOption {
    /// What to show for the setting: the name of the choice in force, or its value when
    /// the agent gave the choices no names of their own.
    pub fn current_name(&self) -> Option<String> {
        let current = self.current.as_deref()?;
        Some(
            self.choices
                .iter()
                .find(|choice| choice.value == current)
                .map(|choice| choice.name.clone())
                .unwrap_or_else(|| current.to_owned()),
        )
    }
}
/// How much of the model's context the conversation is using, when the agent says.
///
/// ACP does not standardise this, so it is read from `_meta` and every field is optional:
/// a panel that invents a context window is a panel that lies about how much room is left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    /// Tokens spent so far.
    pub used: u64,
    /// The window they are spent from, if the agent names one.
    pub window: Option<u64>,
    /// What the agent's account says, when its update carries it.
    ///
    /// The protocol has no field for money — the update's own `cost` is what this conversation spent —
    /// so an agent that knows a balance puts it in `_meta`, which the protocol reserves for exactly
    /// this. Not every agent sends one, and one that sends nothing says nothing: an account that could
    /// not be read is not an account with no money.
    pub account: Option<AccountUsage>,
}

/// What an agent says about its account, in its provider's own words.
///
/// Lines rather than fields, because that is what a provider reports: `/usage` prints a list of
/// labelled facts, and a Nous top-up, a Codex window and any other provider's balance are all the
/// same list. The panel shows what came, so a provider nobody has written code for yet is still read
/// correctly by the person looking at it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountUsage {
    /// The provider the lines belong to, when the agent names one.
    pub provider: Option<String>,
    /// The labelled lines, as the provider wrote them: `Top-up credits: $14.06`.
    pub lines: Vec<String>,
    /// Whether the agent warns that the lines are old.
    pub stale: bool,
}

/// The account an agent attached to its update (`_meta.hermes.accountUsage`), or `None` when it
/// attached nothing, said nothing, or is not an agent that speaks of accounts at all.
pub fn account_usage_of(message: &serde_json::Value) -> Option<AccountUsage> {
    let block = message.get("_meta")?.get("hermes")?.get("accountUsage")?;
    let lines: Vec<String> = block
        .get("lines")?
        .as_array()?
        .iter()
        .filter_map(|line| line.as_str())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(AccountUsage {
        provider: block
            .get("provider")
            .and_then(|provider| provider.as_str())
            .map(str::to_owned),
        lines,
        stale: block
            .get("stale")
            .and_then(|stale| stale.as_bool())
            .unwrap_or(false),
    })
}
/// The state of a conversation the agent described when it opened it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionState {
    /// The models it offers, in its own order.
    pub models: Vec<Model>,
    /// The one in use.
    pub current_model: Option<String>,
    /// The modes it offers.
    pub modes: Vec<Mode>,
    /// The one in use.
    pub current_mode: Option<String>,
    /// The other settings it offers: ACP's config options, of which the level of
    /// thinking is the one this panel shows.
    pub options: Vec<ConfigOption>,
}
impl SessionState {
    /// The setting for how hard the model thinks, if the agent offers one.
    ///
    /// ACP names it by category; the id is the fallback, and Hermes' id is its own.
    pub fn thought_level(&self) -> Option<&ConfigOption> {
        self.options.iter().find(|option| {
            option.category.as_deref() == Some(THOUGHT_LEVEL) || option.id == REASONING
        })
    }
}
/// Reads `models` and `modes` out of a `session/new` answer.
pub fn session_state(answer: &Value) -> SessionState {
    let mut state = SessionState::default();
    if let Some(models) = answer.get("models") {
        state.current_model = models
            .get("currentModelId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        state.models = models
            .get("availableModels")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|model| {
                        let id = model.get("modelId").and_then(Value::as_str)?;
                        Some(Model {
                            id: id.to_owned(),
                            name: model
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(id)
                                .to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    state.options = options_of(answer);
    if let Some(modes) = answer.get("modes") {
        state.current_mode = modes
            .get("currentModeId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        state.modes = modes
            .get("availableModes")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|mode| {
                        let id = mode.get("id").and_then(Value::as_str)?;
                        Some(Mode {
                            id: id.to_owned(),
                            name: mode
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(id)
                                .to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    state
}
/// A command the agent offers, as `available_commands_update` carries it.
///
/// Hermes names them and says what they do (`acp_adapter/commands.py::_available_commands`):
/// `name`, `description`, and an `input.hint` for the ones that take an argument. A command
/// is sent back as a prompt that starts with its name and a slash — the protocol has no other
/// way — so the panel only has to know what to offer.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentCommand {
    /// The word it is invoked by, without the slash.
    pub name: String,
    /// What it does, in the agent's own words.
    pub description: String,
    /// What it takes after the name, when it takes anything.
    pub hint: Option<String>,
}
/// The first line of a message, as much of it as one aside can hold.
///
/// Public because the panel draws the same fallback for a service row of an agent that did not
/// mark it.
pub fn first_line(text: &str) -> String {
    /// How many characters an aside holds before it stops being one.
    const ROOM: usize = 160;

    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= ROOM {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(ROOM).collect();
    cut.push('…');
    cut
}
/// One of the answers an agent offers when it wants permission to do something.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    /// The id to answer with.
    pub option_id: String,
    /// The label to show, in the agent's words.
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once` or `reject_always`.
    pub kind: String,
}
/// What the agent is asking about: the tool call, as it described it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PermissionRequest {
    /// The session it belongs to.
    pub session: String,
    /// The tool call the agent wants to run.
    pub tool_call: Value,
    /// The ways to answer.
    pub options: Vec<PermissionOption>,
}
impl PermissionRequest {
    /// Reads a `session/request_permission` request.
    pub fn from_params(params: &Value) -> Self {
        let options = params
            .get("options")
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| serde_json::from_value(option.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            session: params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            tool_call: params.get("toolCall").cloned().unwrap_or(Value::Null),
            options,
        }
    }

    /// A title for the question: the tool call's own, when it has one.
    pub fn title(&self) -> String {
        self.tool_call
            .get("title")
            .and_then(Value::as_str)
            .or_else(|| self.tool_call.get("kind").and_then(Value::as_str))
            .unwrap_or("the agent asks for permission")
            .to_owned()
    }

    /// The first option of a kind, for a policy that answers without asking.
    pub fn option_of(&self, kind: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|option| option.kind == kind)
            .map(|option| option.option_id.as_str())
    }
}
/// One variant the agent offers in an elicitation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceOption {
    /// What goes back to the agent when this variant is chosen.
    pub value: String,
    /// What the panel shows for it.
    pub title: String,
}
/// A question the agent asks the person, with variants to choose from.
///
/// ACP's elicitation carries a flat JSON Schema. The editor draws a variant selection,
/// so only the shape it can draw is accepted: one property whose values are an `enum` or
/// a `oneOf` — or an array of those, when several may be chosen at once. An agent that asks
/// for something else gets a refusal in the protocol's own words rather than a form nobody
/// can fill in.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChoiceRequest {
    /// The session it belongs to.
    pub session: String,
    /// What the agent is asking, in its own words.
    pub message: String,
    /// The name of the property the answer goes back under.
    pub field: String,
    /// The variants, in the agent's order.
    pub options: Vec<ChoiceOption>,
    /// Whether several variants may be chosen at once. Hermes asks for several by making the
    /// property an array of the same strings (`acp_adapter/elicitation.py`).
    pub multiple: bool,
    /// Whether the answer is written rather than picked: Hermes asks for free text with a bare
    /// `{"type": "string"}` and no values to choose between.
    pub free_text: bool,
}
impl ChoiceRequest {
    /// Reads an `elicitation/create` request in form mode.
    pub fn from_params(params: &Value) -> Self {
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the agent asks you to choose")
            .to_owned();
        let schema = params.get("requestedSchema");
        let properties = schema
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object);
        let (field, options, multiple, free_text) = properties
            .and_then(|properties| properties.iter().next())
            .map(|(field, property)| {
                let options = choice_options_of(property).unwrap_or_default();
                let kind = property.get("type").and_then(Value::as_str);
                let multiple = kind == Some("array");
                // A string — or an array of strings — with nothing to choose between is a
                // question to be written out. A property of any other shape is one this editor
                // cannot draw, and then the card offers nothing but a refusal.
                let free_text =
                    options.is_empty() && matches!(kind, Some("string") | Some("array"));
                (field.clone(), options, multiple, free_text)
            })
            .unwrap_or_default();
        Self {
            session: params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            message,
            field,
            options,
            multiple,
            free_text,
        }
    }

    /// A title for the question.
    pub fn title(&self) -> &str {
        &self.message
    }
}
/// The variants a property allows, as `(value, title)`.
fn choice_options_of(property: &Value) -> Option<Vec<ChoiceOption>> {
    // Several answers to choose between are one level down: the same shape the single answer
    // has, as the items of an array.
    if property.get("type").and_then(Value::as_str) == Some("array") {
        return property.get("items").and_then(choice_options_of);
    }
    if let Some(values) = property.get("enum").and_then(Value::as_array) {
        return Some(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(|value| ChoiceOption {
                    value: value.to_owned(),
                    title: value.to_owned(),
                })
                .collect(),
        );
    }
    let options = property.get("oneOf").and_then(Value::as_array)?;
    Some(
        options
            .iter()
            .filter_map(|option| {
                let value = option.get("const").and_then(Value::as_str)?;
                Some(ChoiceOption {
                    value: value.to_owned(),
                    title: option
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or(value)
                        .to_owned(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod account_usage_tests {
    use super::*;

    #[test]
    fn an_account_the_agent_attaches_is_read_in_its_providers_own_words() {
        // What Hermes sends on a usage update: `_meta.hermes.accountUsage`, the lines `/usage` prints.
        let update = serde_json::json!({
            "sessionUpdate": "usage_update",
            "used": 527478,
            "size": 1000000,
            "_meta": {"hermes": {"accountUsage": {
                "provider": "nous",
                "lines": ["Nous credits", "Provider: nous", "Top-up credits: $14.06"],
                "stale": false
            }}}
        });
        let account = account_usage_of(&update).expect("the account was attached and not read");
        assert_eq!(account.provider.as_deref(), Some("nous"));
        assert_eq!(account.lines[2], "Top-up credits: $14.06");
        assert!(!account.stale);
    }

    #[test]
    fn an_update_without_an_account_says_nothing_of_one() {
        let plain = serde_json::json!({"sessionUpdate": "usage_update", "used": 1, "size": 2});
        assert_eq!(account_usage_of(&plain), None);
        // Attached and empty is the same as not attached: nothing is not zero.
        let empty = serde_json::json!({"_meta": {"hermes": {"accountUsage": {"lines": []}}}});
        assert_eq!(account_usage_of(&empty), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_settings_are_read_the_way_hermes_sends_them() {
        // The shape Hermes puts on the wire (acp_adapter/gateway_session_meta.py): the
        // edit policy and the level of thinking, both as selects.
        let answer = json!({
            "configOptions": [
                {
                    "id": "edit_approval_policy",
                    "name": "Edits",
                    "category": "mode",
                    "type": "select",
                    "currentValue": "ask",
                    "options": [
                        {"value": "ask", "name": "Default"},
                        {"value": "session", "name": "Don't Ask"}
                    ]
                },
                {
                    "id": "reasoning_effort",
                    "name": "Reasoning effort",
                    "category": "thought_level",
                    "type": "select",
                    "currentValue": "medium",
                    "options": [
                        {"value": "low", "name": "low"},
                        {"value": "medium", "name": "medium"}
                    ]
                }
            ]
        });
        let options = options_of(&answer);
        assert_eq!(options.len(), 2, "the settings were not read: {options:?}");
        let level = options
            .iter()
            .find(|option| option.category.as_deref() == Some(THOUGHT_LEVEL))
            .expect("the level of thinking is not among the settings");
        assert_eq!(level.id, "reasoning_effort");
        assert_eq!(
            level.current_name().as_deref(),
            Some("medium"),
            "the choice in force is not named"
        );
        assert_eq!(
            options[0].current_name().as_deref(),
            Some("Default"),
            "the name the agent gave the choice was not used"
        );
    }
    #[test]
    fn a_session_answer_carries_the_settings_into_its_state() {
        let answer = json!({
            "sessionId": "s1",
            "configOptions": [{
                "id": "reasoning_effort",
                "name": "Reasoning effort",
                "category": "thought_level",
                "currentValue": "high",
                "options": [{"value": "high", "name": "high"}]
            }]
        });
        let state = session_state(&answer);
        let level = state
            .thought_level()
            .expect("the level is not in the state the session was opened with");
        assert_eq!(level.current.as_deref(), Some("high"));
        // An agent that offers no such setting offers no level: the state says so by
        // having nothing to find.
        assert!(session_state(&json!({})).thought_level().is_none());
    }
    #[test]
    fn a_permission_request_offers_its_options() {
        let params = json!({
            "sessionId": "s1",
            "toolCall": {"toolCallId": "t1", "title": "Run the tests"},
            "options": [
                {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                {"optionId": "no", "name": "Reject", "kind": "reject_once"}
            ]
        });
        let request = PermissionRequest::from_params(&params);
        assert_eq!(request.title(), "Run the tests");
        assert_eq!(request.option_of("allow_once"), Some("yes"));
        assert_eq!(request.option_of("reject_always"), None);
    }
    #[test]
    fn an_elicitation_offers_its_variants() {
        let params = json!({
            "sessionId": "s1",
            "mode": "form",
            "message": "Which variant should I write?",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "variant": {
                        "type": "string",
                        "enum": ["compact", "readable", "fast"]
                    }
                },
                "required": ["variant"]
            }
        });
        let request = ChoiceRequest::from_params(&params);
        assert_eq!(request.title(), "Which variant should I write?");
        assert_eq!(request.field, "variant");
        assert_eq!(
            request
                .options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["compact", "readable", "fast"]
        );
    }
    #[test]
    fn a_choice_of_several_is_read_and_answered_as_a_list() {
        // Hermes 0.22.5, `acp_adapter/elicitation.py::build_clarify_elicitation`: a question
        // of several is the shape of a question of one, one level down — an array of the same
        // strings. A client that only knew the flat shape saw the question and none of its
        // answers, which is what a multi-select dialog looked like.
        let params = json!({
            "sessionId": "editor-session",
            "mode": "form",
            "message": "Which of these should I do?",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "answer": {
                        "type": "array",
                        "items": {"type": "string", "enum": ["tests", "docs", "release"]}
                    }
                },
                "required": ["answer"]
            }
        });
        let request = ChoiceRequest::from_params(&params);
        assert!(request.multiple, "a question of several was read as one");
        assert_eq!(request.field, "answer");
        assert_eq!(
            request
                .options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["tests", "docs", "release"]
        );

        // What goes back for such a question is a list, because the agent asked for a list, and a
        // refusal is a different thing from an empty list: both are asked after in
        // `tests/sdk_session.rs`, where a question is answered through the connection itself.
    }

    #[test]
    fn the_hermes_form_is_read_as_it_is_sent() {
        // The exact shape Hermes 0.22.5 puts on the wire
        // (acp_adapter/elicitation.py::build_clarify_elicitation): one
        // "answer" property whose enum is the offered choices.
        let params = json!({
            "sessionId": "editor-session",
            "mode": "form",
            "message": "Which variant should I write?",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "answer": {
                        "type": "string",
                        "enum": ["short", "readable", "formal"]
                    }
                },
                "required": ["answer"]
            }
        });
        let request = ChoiceRequest::from_params(&params);
        assert_eq!(request.session, "editor-session");
        assert_eq!(request.field, "answer");
        assert_eq!(
            request
                .options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["short", "readable", "formal"]
        );
    }
    #[test]
    fn a_question_to_be_written_out_is_read_as_one() {
        // Hermes asks for free text with a bare string and no values to choose between
        // (`acp_adapter/elicitation.py`: the schema stays `{"type": "string"}` when the agent
        // offered no choices). There is nothing to click, so the card has to offer somewhere
        // to write — a card with a heading and a refusal is a question nobody can answer.
        let params = json!({
            "sessionId": "editor-session",
            "mode": "form",
            "message": "What should I call it?",
            "requestedSchema": {
                "type": "object",
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"]
            }
        });
        let request = ChoiceRequest::from_params(&params);
        assert!(
            request.free_text,
            "a question to be written out was read as one to pick"
        );
        assert!(request.options.is_empty(), "there are no variants to offer");
        assert!(!request.multiple);
        assert_eq!(request.field, "answer");

        // And the answer goes back as the words themselves, not as a list.

        // A property of some other shape is still one this editor cannot draw: no field, no
        // variants, and only the refusal is left.
        let odd = ChoiceRequest::from_params(&json!({
            "sessionId": "editor-session",
            "mode": "form",
            "message": "How many?",
            "requestedSchema": {
                "type": "object",
                "properties": {"answer": {"type": "integer"}}
            }
        }));
        assert!(!odd.free_text, "a number was read as something to write");
        assert!(odd.options.is_empty());
    }

    #[test]
    fn a_change_of_settings_is_read_as_the_whole_list() {
        // The shape Hermes puts on the wire when a setting changes: the whole list, not the one
        // that moved — and the panel shows what the agent said, not what was asked of it.
        let update = json!({
            "configOptions": [{
                "id": "reasoning_effort",
                "name": "Reasoning effort",
                "category": "thought_level",
                "currentValue": "xhigh",
                "options": [{"value": "xhigh", "name": "xhigh"}]
            }]
        });
        let options = options_of(&update);
        assert_eq!(options.len(), 1, "the list did not arrive whole");
        assert_eq!(options[0].current.as_deref(), Some("xhigh"));
    }
}
