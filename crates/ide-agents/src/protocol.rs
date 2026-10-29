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

/// One mode the agent offers: in practice, how hard it should think.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    /// What to send back in `session/set_mode`.
    pub id: String,
    /// What to show a person.
    pub name: String,
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

/// Reads `_meta.usage` from anything that can carry it: a prompt's answer, an update.
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
            "agent_message_chunk" | "user_message_chunk" => {
                let text = update
                    .get("content")
                    .and_then(|content| content.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Update::Text(text.to_owned())
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
/// a `oneOf`. An agent that asks for something else gets a refusal in the protocol's own
/// words rather than a form nobody can fill in.
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
        let (field, options) = properties
            .and_then(|properties| properties.iter().next())
            .map(|(field, property)| {
                (
                    field.clone(),
                    options_of(property).unwrap_or_else(|| {
                        // A single string with an enum is the common shape; a bare
                        // property the editor cannot draw leaves only a refusal.
                        Vec::new()
                    }),
                )
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
        }
    }

    /// A title for the question.
    pub fn title(&self) -> &str {
        &self.message
    }
}

/// The variants a property allows, as `(value, title)`.
fn options_of(property: &Value) -> Option<Vec<ChoiceOption>> {
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

/// The answer to an elicitation: the chosen variant, or the person's refusal.
pub fn choice_answer(field: &str, choice: Option<&str>) -> Value {
    match choice {
        Some(value) => json!({
            "action": "accept",
            "content": { field: value }
        }),
        None => json!({"action": "decline"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            choice_answer("variant", Some("readable")),
            json!({"action": "accept", "content": {"variant": "readable"}})
        );
        assert_eq!(choice_answer("variant", None), json!({"action": "decline"}));
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
            choice_answer(&request.field, Some("readable")),
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
