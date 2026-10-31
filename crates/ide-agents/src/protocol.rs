//! The words of the Agent Client Protocol, and nothing else.
//!
//! ACP is JSON-RPC 2.0 with one message per line over the agent's standard input and
//! output. This module holds the messages in both directions; it knows how to read them
//! and how to write them, and it knows nothing about processes, threads or the editor.
//!
//! Everything that the protocol defines but this client does not yet act on is kept as
//! `serde_json::Value` rather than dropped: a newer agent will send fields this version
//! has never heard of, and refusing the message because of them would be a version of
//! the protocol that only works with agents built the same week as the editor.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The version of ACP this client speaks. Agents answer with the version they will use,
/// which may be older.
pub const PROTOCOL_VERSION: u64 = 1;

/// A message from one side to the other.
///
/// The three shapes are told apart by which fields are present, not by a tag: a request
/// has an id and a method, a notification has a method and no id, a response has an id
/// and a result or an error. That is what JSON-RPC 2.0 says, and agents on the other
/// side of the wire implement it that way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    /// A method call that expects an answer.
    Request {
        jsonrpc: String,
        id: u64,
        method: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<Value>,
    },
    /// A method call that expects no answer.
    Notification {
        jsonrpc: String,
        method: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<Value>,
    },
    /// The answer to a request, whether it succeeded or not.
    Response {
        jsonrpc: String,
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<RpcError>,
    },
}

impl Message {
    /// A request, numbered by the caller.
    pub fn request(id: u64, method: &str, params: Value) -> Self {
        Message::Request {
            jsonrpc: "2.0".to_owned(),
            id,
            method: method.to_owned(),
            params: Some(params),
        }
    }

    /// A notification: a method call with no answer.
    pub fn notification(method: &str, params: Value) -> Self {
        Message::Notification {
            jsonrpc: "2.0".to_owned(),
            method: method.to_owned(),
            params: Some(params),
        }
    }

    /// A successful answer.
    pub fn result(id: u64, result: Value) -> Self {
        Message::Response {
            jsonrpc: "2.0".to_owned(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A refused answer.
    pub fn failure(id: u64, code: i64, message: &str) -> Self {
        Message::Response {
            jsonrpc: "2.0".to_owned(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.to_owned(),
            }),
        }
    }

    /// One line of JSON, without the newline: the protocol is line-delimited.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned())
    }
}

/// A JSON-RPC error, in the words the other side used.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// The JSON-RPC code: `-32601` for a method that does not exist, and so on.
    pub code: i64,
    /// What went wrong, for a person to read.
    pub message: String,
}

/// What this client can do, sent with `initialize`.
///
/// Claiming a capability is a promise to answer the matching request: an agent that
/// reads files through the client is doing it because the client said it could.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    /// Whether `fs/read_text_file` will be answered.
    pub fs: FsCapabilities,
    /// Whether `terminal/*` will be answered. Not yet, and saying so is the point.
    pub terminal: bool,
    /// Whether `elicitation/create` will be answered: the agent may ask the person to
    /// pick one of several variants, and the panel draws the choices.
    pub elicitation: ElicitationCapabilities,
}

/// The kinds of user interaction the client can present.
///
/// ACP requires each supported mode to be named explicitly: a present but empty object
/// advertises nothing. Only form mode is offered — a URL would send the person out of the
/// editor, and a variant to choose from is a form.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationCapabilities {
    /// Present when `elicitation/create` in form mode will be answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form: Option<FormCapabilities>,
}

impl Default for ElicitationCapabilities {
    fn default() -> Self {
        Self {
            form: Some(FormCapabilities::default()),
        }
    }
}

/// Form mode, which takes no settings of its own: its presence is the promise.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FormCapabilities {}

/// What the client will do with the file system.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCapabilities {
    /// Whether `fs/read_text_file` will be answered.
    pub read_text_file: bool,
    /// Whether `fs/write_text_file` will be answered. Writes go through the editor, so
    /// this is a promise only once the panel asks for confirmation.
    pub write_text_file: bool,
}

impl Default for FsCapabilities {
    fn default() -> Self {
        Self {
            read_text_file: true,
            write_text_file: false,
        }
    }
}

/// The `initialize` parameters.
pub fn initialize_params(capabilities: &ClientCapabilities) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": capabilities,
    })
}

/// The `session/load` parameters: the conversation, and where it lives.
///
/// ACP asks for the same three things as `session/new`, because an agent that kept a
/// session kept it against a directory: the same id in another project is not the same
/// conversation.
pub fn load_session_params(cwd: &std::path::Path, session: &str) -> Value {
    json!({
        "sessionId": session,
        "cwd": cwd.to_string_lossy(),
        "mcpServers": Vec::<Value>::new(),
    })
}

/// The `session/list` parameters: the page to continue from, and which directory's conversations
/// are wanted. A client that names its project asks for the conversations of that project, not for
/// every conversation on the machine.
pub fn list_sessions_params(cwd: Option<&std::path::Path>, cursor: Option<&str>) -> Value {
    let mut params = serde_json::Map::new();
    if let Some(cwd) = cwd {
        params.insert("cwd".to_owned(), Value::String(cwd.display().to_string()));
    }
    if let Some(cursor) = cursor {
        params.insert("cursor".to_owned(), Value::String(cursor.to_owned()));
    }
    Value::Object(params)
}

/// One conversation the agent knows of, as `session/list` describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionInfo {
    /// The id to hand back to `session/load`.
    pub id: String,
    /// Which surface the conversation was had on, when it is known — `telegram`, `desktop`,
    /// `cli`, `acp`. ACP itself never says; the agent does when it is asked for the list.
    pub source: Option<String>,
    /// Where that conversation lives, if the agent says.
    pub cwd: Option<String>,
    /// What the agent calls it, if it names it at all.
    pub title: Option<String>,
    /// When it was last touched, in the agent's own words — a string, because agents differ and
    /// a date nobody can parse is still worth showing.
    pub updated_at: Option<String>,
}

/// The words sent to the agent when the panel wants the list of conversations it keeps.
///
/// ACP's own `session/list` is filtered by the adapter that answers it (Hermes' ACP adapter asks
/// its database for `source="acp"`, so only conversations started from an editor appear), and no
/// request can widen it. The agent itself is not bound by that: it keeps the store, and can be
/// asked to read it and answer with what is in it. The answer is asked for as JSON so that it can
/// be read rather than guessed at.
pub fn list_sessions_prompt(limit: usize) -> String {
    format!(
        "Answer with JSON only, no prose and no code fence: list my Hermes sessions from the \
         sessions store (the `sessions` table of state.db), newest first, as a JSON array of \
         objects with keys \"id\", \"title\", \"updated_at\" and \"source\". Include every \
         surface — telegram, desktop, tui, cli, acp — and at most {limit} rows. If you cannot \
         read the store, answer with an empty array."
    )
}

/// Reads the conversations out of the agent's answer: the first JSON array in the text, read as
/// rows of the four keys the prompt asks for. Anything else in the answer is ignored — an agent
/// that explains itself first is still answering.
pub fn sessions_from_text(text: &str) -> Vec<SessionInfo> {
    let Some(start) = text.find('[') else {
        return Vec::new();
    };
    // The array may be fenced or followed by prose: take the longest run that parses.
    let tail = &text[start..];
    let end = match tail.rfind(']') {
        Some(end) => &tail[..=end],
        None => tail,
    };
    let Ok(rows) = serde_json::from_str::<Value>(end) else {
        return Vec::new();
    };
    let Some(rows) = rows.as_array() else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let id = row
                .get("id")
                .or_else(|| row.get("sessionId"))
                .and_then(Value::as_str)?
                .to_owned();
            Some(SessionInfo {
                id,
                cwd: row.get("cwd").and_then(Value::as_str).map(str::to_owned),
                title: row.get("title").and_then(Value::as_str).map(str::to_owned),
                updated_at: row
                    .get("updated_at")
                    .or_else(|| row.get("updatedAt"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                source: row.get("source").and_then(Value::as_str).map(str::to_owned),
            })
        })
        .collect()
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
                        source: row.get("source").and_then(Value::as_str).map(str::to_owned),
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

/// The `session/new` parameters. `mcpServers` is empty: the editor brings no servers of
/// its own, and an agent that wants one can start it itself.
///
/// The client serves no `fs/*` and no `terminal/*`, because the agent's own terminal is
/// where its project actually is — inside a container, on a build host, wherever it was
/// configured to work. Saying that in `initialize` alone is easy to miss; saying it once
/// more, next to the directory, is how an agent hears it: read and write files through
/// your own terminal, in the directory named here.
pub fn new_session_params(cwd: &std::path::Path) -> Value {
    json!({
        "cwd": cwd.to_string_lossy(),
        "mcpServers": Vec::<Value>::new(),
        "_meta": {
            "client": "andy-ide",
            "fileAccess": "terminal",
            "note": "Files are not read or written for you by this client: use your own \
                     terminal for every file operation, in the cwd above."
        },
    })
}

/// The `session/prompt` parameters: one block of text, which is all a chat field sends.
pub fn prompt_params(session: &str, text: &str) -> Value {
    json!({
        "sessionId": session,
        "prompt": [{"type": "text", "text": text}],
    })
}

/// One model the agent offers, as `session/new` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    /// What to send back in `session/set_model`.
    pub id: String,
    /// What to show a person.
    pub name: String,
}

/// Reads the settings an agent offers out of a session answer, or out of the reply to a
/// change: ACP puts the whole list in `configOptions` either way.
/// The notification that asks the agent to stop the turn in flight.
///
/// A notification, not a request: ACP has no answer to it — a stopped turn ends with
/// `stopReason: cancelled`, and that arrives as the turn's own ending.
pub fn cancel_params(session: &str) -> Value {
    json!({ "sessionId": session })
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

/// The `session/set_config_option` parameters: the setting, and the choice picked.
pub fn config_option_params(session: &str, config: &str, value: &str) -> Value {
    json!({
        "sessionId": session,
        "configId": config,
        "value": value,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    /// Tokens spent so far.
    pub used: u64,
    /// The window they are spent from, if the agent names one.
    pub window: Option<u64>,
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

/// Reads the commands an `available_commands_update` carries.
pub fn commands_of(update: &Value) -> Vec<AgentCommand> {
    update
        .get("availableCommands")
        .and_then(Value::as_array)
        .map(|commands| {
            commands
                .iter()
                .filter_map(|command| {
                    Some(AgentCommand {
                        name: command.get("name").and_then(Value::as_str)?.to_owned(),
                        description: command
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        hint: command
                            .get("input")
                            .and_then(|input| input.get("hint"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the `usage_update` a `session/update` carries: Hermes sends it at the end of a turn
/// (`acp_adapter/server.py::_build_usage_update`), with `size` the context window and `used`
/// what has been spent from it. Before this was read, the panel showed no context figure at
/// all with a real agent — the figure was there, under a name we did not look for.
pub fn usage_update_of(update: &Value) -> Option<Usage> {
    let used = ["used", "totalTokens", "input_tokens"]
        .iter()
        .find_map(|key| update.get(*key).and_then(Value::as_u64))?;
    let window = ["size", "window", "contextWindow"]
        .iter()
        .find_map(|key| update.get(*key).and_then(Value::as_u64));
    Some(Usage { used, window })
}

/// Reads `_meta.usage` from anything that can carry it: a prompt's answer, an update.
///
/// This is the shape an agent may put beside an answer. Hermes does not: it sends a
/// `usage_update` of its own, which [`usage_update_of`] reads — `used` and `size`, the
/// window and what is spent from it.
pub fn usage_of(value: &Value) -> Option<Usage> {
    let usage = value.get("_meta")?.get("usage")?;
    let used = ["used", "totalTokens", "inputTokens"]
        .iter()
        .find_map(|key| usage.get(*key).and_then(Value::as_u64))?;
    let window = ["window", "contextWindow", "maxTokens"]
        .iter()
        .find_map(|key| usage.get(*key).and_then(Value::as_u64));
    Some(Usage { used, window })
}

/// The request that changes the model mid-conversation.
pub fn set_model_params(session: &str, model: &str) -> Value {
    json!({"sessionId": session, "modelId": model})
}

/// The request that changes the mode — how hard the agent thinks — mid-conversation.
pub fn set_mode_params(session: &str, mode: &str) -> Value {
    json!({"sessionId": session, "modeId": mode})
}

/// One thing the agent wants the editor to know during a turn.
///
/// `session/update` carries a tagged union whose tag is `sessionUpdate`, and the variants
/// grow with the protocol. The ones this client shows are typed; the rest are kept as
/// they arrived so that nothing is lost before there is somewhere to put it.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// A piece of the agent's answer, to be appended as it arrives.
    Text(String),
    /// The person's own words, as a restored session replays them. The other side of a
    /// conversation belongs in the transcript too: a panel that drew every replay as the
    /// agent's answer would read as a conversation that was all answers.
    You(String),
    /// A piece of the agent's reasoning: shown folded away, because it is the working
    /// out and not the answer.
    Thinking(String),
    /// A tool call the agent has started, changed or finished.
    ToolCall {
        /// The id the agent uses for it in later updates.
        id: String,
        /// Its title, as the agent words it.
        title: String,
        /// `pending`, `in_progress`, `completed` or `failed`.
        status: String,
    },
    /// The plan the agent is following, as a list of steps.
    Plan(Vec<String>),
    /// The settings on offer, whole, after one of them changed.
    Settings(Vec<ConfigOption>),
    /// How much of its window the agent has spent: the figure a context indicator shows.
    Usage(Usage),
    /// The commands the agent offers, whole, after it has said what they are.
    Commands(Vec<AgentCommand>),
    /// Anything this version does not show yet.
    Other {
        /// The tag, for the record.
        kind: String,
        /// The whole update, so nothing is thrown away.
        body: Value,
    },
}

impl Update {
    /// Reads the `update` object of a `session/update` notification.
    pub fn from_value(update: &Value) -> Self {
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "agent_thought_chunk" => {
                let text = update
                    .get("content")
                    .and_then(|content| content.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Update::Thinking(text.to_owned())
            }
            // A message chunk is one complete message of the conversation: the agent's
            // answer, or the person's own words when a session replays its history
            // (`acp_adapter/server.py::_history_replay_updates` sends each role as its own
            // chunk). The two are drawn differently, so they are read apart.
            "agent_message_chunk" => {
                let text = update
                    .get("content")
                    .and_then(|content| content.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Update::Text(text.to_owned())
            }
            "user_message_chunk" => {
                let text = update
                    .get("content")
                    .and_then(|content| content.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Update::You(text.to_owned())
            }
            "tool_call" | "tool_call_update" => Update::ToolCall {
                id: update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                title: update
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                status: update
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("pending")
                    .to_owned(),
            },
            "plan" => {
                let steps = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|entry| {
                                entry
                                    .get("content")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Update::Plan(steps)
            }
            "config_option_update" => Update::Settings(options_of(update)),
            "available_commands_update" => Update::Commands(commands_of(update)),
            "usage_update" => match usage_update_of(update) {
                Some(usage) => Update::Usage(usage),
                None => Update::Other {
                    kind: kind.to_owned(),
                    body: update.clone(),
                },
            },
            _ => Update::Other {
                kind: kind.to_owned(),
                body: update.clone(),
            },
        }
    }
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

/// The answer to a permission request: a chosen option, or a refusal that names no
/// option at all.
pub fn permission_answer(option: Option<&str>) -> Value {
    match option {
        Some(option_id) => json!({
            "outcome": {"outcome": "selected", "optionId": option_id}
        }),
        None => json!({"outcome": {"outcome": "cancelled"}}),
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

/// The answer to an elicitation: the variant chosen, the variants when several could be, or
/// the person's refusal.
///
/// When several may be chosen the answer is a list of them — a list of one is still a list,
/// because the agent decided the shape of the question, not the editor.
pub fn choice_answer(field: &str, multiple: bool, choice: Option<&[String]>) -> Value {
    match choice {
        Some(values) if multiple => json!({
            "action": "accept",
            "content": { field: values }
        }),
        Some(values) => match values.first() {
            Some(value) => json!({
                "action": "accept",
                "content": { field: value }
            }),
            None => json!({"action": "decline"}),
        },
        None => json!({"action": "decline"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn changing_a_setting_names_the_setting_and_the_choice() {
        assert_eq!(
            config_option_params("s1", "reasoning_effort", "high"),
            json!({"sessionId": "s1", "configId": "reasoning_effort", "value": "high"})
        );
    }

    #[test]
    fn a_setting_that_changed_arrives_as_the_whole_list() {
        let update = json!({
            "sessionUpdate": "config_option_update",
            "configOptions": [{
                "id": "reasoning_effort",
                "name": "Reasoning effort",
                "category": "thought_level",
                "currentValue": "xhigh",
                "options": [{"value": "xhigh", "name": "xhigh"}]
            }]
        });
        match Update::from_value(&update) {
            Update::Settings(options) => {
                assert_eq!(options.len(), 1);
                assert_eq!(options[0].current.as_deref(), Some("xhigh"));
            }
            other => panic!("a change of settings was read as {other:?}"),
        }
    }

    #[test]
    fn a_request_is_read_from_the_line_it_arrived_on() {
        let line =
            r#"{"jsonrpc":"2.0","id":7,"method":"fs/read_text_file","params":{"path":"a.txt"}}"#;
        match serde_json::from_str::<Message>(line).expect("the message") {
            Message::Request { id, method, .. } => {
                assert_eq!(id, 7);
                assert_eq!(method, "fs/read_text_file");
            }
            other => panic!("a request was read as {other:?}"),
        }
    }

    #[test]
    fn a_notification_is_not_a_request() {
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#;
        assert!(matches!(
            serde_json::from_str::<Message>(line).expect("the message"),
            Message::Notification { .. }
        ));
    }

    #[test]
    fn an_answer_carries_either_a_result_or_an_error() {
        let ok = r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s1"}}"#;
        let bad = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"no such method"}}"#;
        match serde_json::from_str::<Message>(ok).expect("the answer") {
            Message::Response { result, error, .. } => {
                assert!(result.is_some());
                assert!(error.is_none());
            }
            other => panic!("{other:?}"),
        }
        match serde_json::from_str::<Message>(bad).expect("the answer") {
            Message::Response { error, .. } => {
                assert_eq!(error.expect("an error").message, "no such method");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_written_message_is_one_line_of_json() {
        let line = Message::request(1, "session/prompt", json!({})).to_line();
        assert!(!line.contains('\n'), "the message spans lines: {line}");
        assert!(line.starts_with(r#"{"jsonrpc":"2.0","id":1,"method":"session/prompt""#));
    }

    #[test]
    fn text_chunks_are_read_as_text() {
        let update = json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "Hello"}
        });
        assert_eq!(
            Update::from_value(&update),
            Update::Text("Hello".to_owned())
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
        assert_eq!(
            choice_answer(
                &request.field,
                request.multiple,
                Some(&["a name of my own".to_owned()])
            ),
            json!({"action": "accept", "content": {"answer": "a name of my own"}})
        );

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
    fn a_user_message_chunk_is_kept_apart_from_the_agent() {
        // A restored session replays the conversation as one chunk per message, and the
        // person's own words are chunks of their own (`acp_adapter/server.py`: the replay
        // sends `user_message_chunk` for role user and `agent_message_chunk` for assistant).
        // Read into the same slot, the words of both sides would draw as the agent's answer —
        // a transcript that restores as if the person had said nothing.
        let user = Update::from_value(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "why is the sky blue?"}
        }));
        assert_eq!(
            user,
            Update::You("why is the sky blue?".to_owned()),
            "the person's own words were read as the agent's"
        );
        let agent = Update::from_value(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "Because air scatters blue light."}
        }));
        assert_eq!(
            agent,
            Update::Text("Because air scatters blue light.".to_owned())
        );
    }

    #[test]
    fn the_commands_are_read_the_way_hermes_sends_them() {
        // Hermes 0.22.5, `acp_adapter/commands.py::_available_commands`: an
        // `available_commands_update` whose commands carry a name, a description, and — for
        // the ones that take something — an input hint.
        let update = json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [
                {"name": "help", "description": "List available commands"},
                {"name": "model",
                 "description": "Show current model and provider, or switch models",
                 "input": {"hint": "model name to switch to"}}
            ]
        });
        match Update::from_value(&update) {
            Update::Commands(commands) => {
                assert_eq!(commands.len(), 2);
                assert_eq!(commands[0].name, "help");
                assert_eq!(commands[0].description, "List available commands");
                assert_eq!(commands[0].hint, None);
                assert_eq!(commands[1].name, "model");
                assert_eq!(commands[1].hint.as_deref(), Some("model name to switch to"));
            }
            other => panic!("the commands were not read: {other:?}"),
        }
    }

    #[test]
    fn the_context_figure_is_read_the_way_hermes_sends_it() {
        // Hermes 0.22.5, `acp_adapter/server.py::_build_usage_update`: a `usage_update` with
        // `size` the context window and `used` what is spent from it. Not `_meta.usage`,
        // which is where this client used to look — and so it showed no figure at all with a
        // real agent.
        let update = json!({"sessionUpdate": "usage_update", "size": 100000, "used": 42000});
        match Update::from_value(&update) {
            Update::Usage(usage) => assert_eq!(
                usage,
                Usage {
                    used: 42000,
                    window: Some(100000)
                }
            ),
            other => panic!("the context figure was not read: {other:?}"),
        }

        // An agent that does not name a window is read as far as it goes.
        match Update::from_value(&json!({"sessionUpdate": "usage_update", "used": 42000})) {
            Update::Usage(usage) => assert_eq!(usage.window, None),
            other => panic!("the figure was dropped for want of a window: {other:?}"),
        }

        // An update with no figure in it stays what it is: something this version does not
        // show, kept whole.
        assert!(matches!(
            Update::from_value(&json!({"sessionUpdate": "usage_update", "note": "no figures"})),
            Update::Other { .. }
        ));
    }

    #[test]
    fn an_unknown_update_is_kept_rather_than_dropped() {
        let update = json!({"sessionUpdate": "something_new", "detail": 42});
        match Update::from_value(&update) {
            Update::Other { kind, body } => {
                assert_eq!(kind, "something_new");
                assert_eq!(body["detail"], json!(42));
            }
            other => panic!("{other:?}"),
        }
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
    fn a_refusal_names_no_option() {
        assert_eq!(
            permission_answer(None),
            json!({"outcome": {"outcome": "cancelled"}})
        );
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
        assert_eq!(
            choice_answer("variant", false, Some(&["readable".to_owned()])),
            json!({"action": "accept", "content": {"variant": "readable"}})
        );
        assert_eq!(
            choice_answer("variant", false, None),
            json!({"action": "decline"})
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

        // The answer goes back as a list, because the agent asked for a list. A refusal is
        // `None` and is a different thing from an empty list.
        let picked = vec!["tests".to_owned(), "docs".to_owned()];
        assert_eq!(
            choice_answer(&request.field, request.multiple, Some(&picked)),
            json!({"action": "accept", "content": {"answer": ["tests", "docs"]}})
        );
        assert_eq!(
            choice_answer(&request.field, request.multiple, None),
            json!({"action": "decline"})
        );
        // One answer to a question of several is still a list of one.
        assert_eq!(
            choice_answer(&request.field, request.multiple, Some(&["docs".to_owned()])),
            json!({"action": "accept", "content": {"answer": ["docs"]}})
        );
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
        assert_eq!(
            choice_answer(&request.field, false, Some(&["readable".to_owned()])),
            json!({"action": "accept", "content": {"answer": "readable"}})
        );
    }

    #[test]
    fn a_choice_can_be_named_with_titles() {
        let params = json!({
            "mode": "form",
            "message": "Pick one",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "variant": {
                        "type": "string",
                        "oneOf": [
                            {"const": "a", "title": "The first way"},
                            {"const": "b", "title": "The second way"}
                        ]
                    }
                }
            }
        });
        let request = ChoiceRequest::from_params(&params);
        assert_eq!(request.options[0].value, "a");
        assert_eq!(request.options[0].title, "The first way");
        assert_eq!(request.options[1].title, "The second way");
    }

    #[test]
    fn the_client_says_it_can_ask_the_person() {
        let capabilities = ClientCapabilities::default();
        let params = initialize_params(&capabilities);
        assert_eq!(
            params["clientCapabilities"]["elicitation"],
            json!({"form": {}}),
            "elicitation form mode was not advertised: {params}"
        );
    }
}
