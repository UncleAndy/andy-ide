//! A stand-in agent, so the client can be tested without one installed.
//!
//! It speaks ACP over standard input and output, one JSON-RPC message per line, and does
//! exactly what the word in the prompt asks for. That is the whole point: a test can say
//! "ask permission" or "read a file" and check what the client did, without a real model,
//! a network, or a syntax for prompting.
//!
//! - `hello` — two chunks, then `end_turn`.
//! - `ask` — asks permission (with `allow_once` and `reject_once`), then says whether it
//!   was allowed.
//! - `choose` — offers three variants and says which one the person picked.
//! - `read` — asks the client for `a.txt`, and answers with what it was given.
//! - `write` — asks the client to write `written.txt`, and says whether it could.
//! - `slow` — waits five seconds before answering.
//! - `die` — leaves without answering, the way a crashed agent does.
//! - anything else — echoes the words back as a single chunk.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// Ends a turn, saying how much of the window it used: ACP has no field for this, so the
/// figure rides in `_meta`, the way a real agent would send it.
fn turn_answer(session: &str, id: Option<u64>) {
    // The context figure the way Hermes sends it at the end of a turn: a `usage_update`
    // notification, `size` the window and `used` what is spent from it
    // (`acp_adapter/server.py::_build_usage_update`). Not in the answer's `_meta` — that
    // is a shape no agent on the wire has been seen to use.
    send(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session,
            "update": {"sessionUpdate": "usage_update", "used": 42000, "size": 100000}
        }
    }));
    answer(id, json!({"stopReason": "end_turn"}));
}

/// The models the stand-in offers.
///
/// Four of them, spread over three providers, so a test can see the grouping. With
/// `ACP_TEST_MANY_MODELS` in its environment it offers a list far longer than a window,
/// which is what a test of scrolling needs.
fn models_offered() -> Value {
    if std::env::var_os("ACP_TEST_MANY_MODELS").is_none() {
        return json!([
            {"modelId": "small", "name": "Small"},
            {"modelId": "large", "name": "Large"},
            {"modelId": "nous/hermes-4", "name": "Nous \u{b7} Hermes 4"},
            {"modelId": "openai/gpt-5", "name": "OpenAI \u{b7} GPT-5"}
        ]);
    }
    let mut offered = vec![
        json!({"modelId": "small", "name": "Small"}),
        json!({"modelId": "large", "name": "Large"}),
    ];
    for index in 0..14 {
        offered.push(
            json!({"modelId": format!("nous/model-{index}"), "name": format!("Nous {index}")}),
        );
        offered.push(
            json!({"modelId": format!("openai/model-{index}"), "name": format!("OpenAI {index}")}),
        );
    }
    Value::Array(offered)
}

/// The levels Hermes offers for how hard a model thinks, in its own words.
///
/// ACP carries them as a *setting* (`thought_level`), not as a mode: the mode channel is the
/// edit policy, which is why the two are separate lists here, as they are in Hermes.
const EFFORTS: [&str; 9] = [
    "default", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

/// Whether the stand-in offers the level at all.
///
/// `--no-levels` makes it behave like Hermes 0.22.5 over ACP, which offers the edit policy
/// and nothing else: a client has to say so rather than offer a menu the agent refuses.
fn levels_offered() -> bool {
    !std::env::args().any(|arg| arg == "--no-levels")
}

/// The mode ACP names for an edit policy: the two channels carry the same choice in
/// Hermes, and the stand-in keeps them in step as Hermes does.
fn policy_mode(policy: &str) -> &'static str {
    match policy {
        "workspace_session" => "accept_edits",
        "session" => "dont_ask",
        _ => "default",
    }
}

/// The edit policy a mode stands for.
fn mode_policy(mode: &str) -> &'static str {
    match mode {
        "accept_edits" => "workspace_session",
        "dont_ask" => "session",
        _ => "ask",
    }
}

/// The settings on offer, as ACP's `configOptions`.
fn settings(effort: &str, policy: &str) -> Value {
    let mut options = vec![json!({
        "id": "edit_approval_policy",
        "name": "Edits",
        "category": "mode",
        "type": "select",
        "currentValue": policy,
        "options": [
            {"value": "ask", "name": "Default"},
            {"value": "workspace_session", "name": "Accept Edits"},
            {"value": "session", "name": "Don't Ask"}
        ]
    })];
    if levels_offered() {
        options.push(json!({
            "id": "reasoning_effort",
            "name": "Reasoning effort",
            "category": "thought_level",
            "type": "select",
            "currentValue": effort,
            "options": EFFORTS
                .iter()
                .map(|level| json!({"value": level, "name": level}))
                .collect::<Vec<_>>()
        }));
    }
    Value::Array(options)
}

fn main() {
    // One reader for the whole program: two `lines()` iterators over the same input
    // each keep a buffer of their own, and the answer to a request of ours can land in
    // the buffer nobody is looking at.
    let stdin = std::io::stdin();
    let mut input = stdin.lock().lines();
    let mut next_id = 100;
    // What the conversation is set to, so a test can see a switch reach the agent.
    let mut model = "small".to_owned();
    // The edit policy, which ACP calls the mode, and the level of thinking, which it calls
    // a setting. Hermes' words for both.
    let mut policy = "ask".to_owned();
    let mut effort = "default".to_owned();
    // The directory the client handed over when it opened the session.
    let mut cwd = String::new();
    // What it claimed it would do for us, and what it said about files.
    let mut caps = String::new();
    // The conversation it is in, and whether it was resumed rather than started.
    let mut session_name = "stand-in-session".to_owned();
    let mut resumed = false;
    while let Some(Ok(line)) = input.next() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let id = message.get("id").and_then(Value::as_u64);

        match method.as_str() {
            "initialize" => {
                caps = message
                    .get("params")
                    .and_then(|params| params.get("clientCapabilities"))
                    .map(|capabilities| capabilities.to_string())
                    .unwrap_or_default();
                // An agent that can be asked to pick up a conversation it kept: without this
                // the client has no business sending `session/load`.
                answer(
                    id,
                    json!({
                        "protocolVersion": 1,
                        "agentCapabilities": {"loadSession": true}
                    }),
                );
            }
            // A conversation kept from an earlier run: the client names it, the agent picks
            // it up. A name it does not know is refused, and the client falls back.
            "session/load" => {
                let wanted = message
                    .get("params")
                    .and_then(|params| params.get("sessionId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                cwd = message
                    .get("params")
                    .and_then(|params| params.get("cwd"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if wanted.starts_with("forgotten") {
                    if let Some(id) = id {
                        send(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32602, "message": "no such session"}
                        }));
                    }
                    continue;
                }
                session_name = wanted;
                resumed = true;
                // A kept conversation is replayed before the load is answered, the way Hermes
                // replays it (`acp_adapter/server.py::_history_replay_updates`): one chunk per
                // message, the person's own words with their own role. A session restored to
                // a panel that only heard the answers would read as half the conversation.
                for (role, said) in [
                    ("user_message_chunk", "why is the sky blue?"),
                    ("agent_message_chunk", "Because air scatters blue light."),
                ] {
                    send(json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": session_name,
                            "update": {
                                "sessionUpdate": role,
                                "content": {"type": "text", "text": said}
                            }
                        }
                    }));
                }
                answer(
                    id,
                    json!({
                        "sessionId": session_name,
                        "models": {
                            "currentModelId": model,
                            "availableModels": models_offered()
                        },
                        "modes": {
                            "currentModeId": policy_mode(&policy),
                            "availableModes": [
                                {"id": "default", "name": "Default"},
                                {"id": "accept_edits", "name": "Accept Edits"},
                                {"id": "dont_ask", "name": "Don't Ask"}
                            ]
                        },
                        "configOptions": settings(&effort, &policy)
                    }),
                );
                // The commands it offers, announced once the session is up, the way Hermes
                // does it (`acp_adapter/commands.py::_send_available_commands_update`): the
                // names and descriptions of the real ones.
                send(json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session_name,
                        "update": {
                            "sessionUpdate": "available_commands_update",
                            "availableCommands": [
                                {"name": "help", "description": "List available commands"},
                                {"name": "model",
                                 "description": "Show current model and provider, or switch models",
                                 "input": {"hint": "model name to switch to"}},
                                {"name": "tools", "description": "List available tools with descriptions"},
                                {"name": "context", "description": "Show conversation message counts by role"},
                                {"name": "reset", "description": "Clear conversation history"},
                                {"name": "compress", "description": "Compress conversation context"},
                                {"name": "steer",
                                 "description": "Inject guidance into the currently running agent turn",
                                 "input": {"hint": "guidance for the active turn"}},
                                {"name": "queue",
                                 "description": "Queue a prompt to run after the current turn finishes",
                                 "input": {"hint": "prompt to run next"}},
                                {"name": "version", "description": "Show Hermes version"}
                            ]
                        }
                    }
                }));
            }
            "session/new" => {
                resumed = false;
                session_name = "stand-in-session".to_owned();
                cwd = message
                    .get("params")
                    .and_then(|params| params.get("cwd"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if let Some(meta) = message.get("params").and_then(|params| params.get("_meta")) {
                    caps = format!("{caps} {meta}");
                }
                // A real agent offers choices here; the stand-in offers two of each so a
                // test can see both menus and watch a switch reach the agent.
                answer(
                    id,
                    json!({
                        "sessionId": session_name,
                        "models": {
                            "currentModelId": model,
                            "availableModels": models_offered()
                        },
                        "modes": {
                            "currentModeId": policy_mode(&policy),
                            "availableModes": [
                                {"id": "default", "name": "Default"},
                                {"id": "accept_edits", "name": "Accept Edits"},
                                {"id": "dont_ask", "name": "Don't Ask"}
                            ]
                        },
                        "configOptions": settings(&effort, &policy)
                    }),
                );
                // The commands it offers, announced once the session is up, the way Hermes
                // does it (`acp_adapter/commands.py::_send_available_commands_update`): the
                // names and descriptions of the real ones.
                send(json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session_name,
                        "update": {
                            "sessionUpdate": "available_commands_update",
                            "availableCommands": [
                                {"name": "help", "description": "List available commands"},
                                {"name": "model",
                                 "description": "Show current model and provider, or switch models",
                                 "input": {"hint": "model name to switch to"}},
                                {"name": "tools", "description": "List available tools with descriptions"},
                                {"name": "context", "description": "Show conversation message counts by role"},
                                {"name": "reset", "description": "Clear conversation history"},
                                {"name": "compress", "description": "Compress conversation context"},
                                {"name": "steer",
                                 "description": "Inject guidance into the currently running agent turn",
                                 "input": {"hint": "guidance for the active turn"}},
                                {"name": "queue",
                                 "description": "Queue a prompt to run after the current turn finishes",
                                 "input": {"hint": "prompt to run next"}},
                                {"name": "version", "description": "Show Hermes version"}
                            ]
                        }
                    }
                }));
            }
            // The settings the client offers to change. The reply carries the whole list
            // read back, the way Hermes answers.
            "session/set_config_option" => {
                let params = message.get("params");
                let config = params
                    .and_then(|params| params.get("configId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let value = params
                    .and_then(|params| params.get("value"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                match config {
                    "reasoning_effort" => effort = value,
                    "edit_approval_policy" => policy = value,
                    _ => {}
                }
                answer(id, json!({"configOptions": settings(&effort, &policy)}));
            }
            "session/set_model" => {
                if let Some(chosen) = message
                    .get("params")
                    .and_then(|params| params.get("modelId"))
                    .and_then(Value::as_str)
                {
                    model = chosen.to_owned();
                }
                answer(id, json!({}));
            }
            "session/set_mode" => {
                if let Some(chosen) = message
                    .get("params")
                    .and_then(|params| params.get("modeId"))
                    .and_then(Value::as_str)
                {
                    policy = mode_policy(chosen).to_owned();
                }
                answer(id, json!({}));
            }
            "session/prompt" => {
                let text = message
                    .get("params")
                    .and_then(|params| params.get("prompt"))
                    .and_then(Value::as_array)
                    .and_then(|prompt| prompt.first())
                    .and_then(|block| block.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let session = message
                    .get("params")
                    .and_then(|params| params.get("sessionId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                match text.trim() {
                    "hello" => {
                        thought(&session, "pondering the greeting");
                        chunk(&session, "Hel");
                        chunk(&session, "lo");
                        turn_answer(&session, id);
                    }
                    "markdown" => {
                        chunk(&session, "**bold** and `code`");
                        turn_answer(&session, id);
                    }
                    "ask" => {
                        next_id += 1;
                        let asked = next_id;
                        request(
                            asked,
                            "session/request_permission",
                            json!({
                                "sessionId": session,
                                "toolCall": {"toolCallId": "t1", "title": "Run the tests"},
                                "options": [
                                    {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                                    {"optionId": "no", "name": "Reject", "kind": "reject_once"}
                                ]
                            }),
                        );
                        let allowed = wait_for_answer(&mut input, asked);
                        let allowed = allowed
                            .and_then(|value| value.get("outcome").cloned())
                            .and_then(|outcome| outcome.get("optionId").cloned())
                            .and_then(|option| option.as_str().map(str::to_owned))
                            .map(|option| option == "yes")
                            .unwrap_or(false);
                        chunk(&session, if allowed { "allowed" } else { "refused" });
                        turn_answer(&session, id);
                    }
                    "ask-text" => {
                        next_id += 1;
                        let asked = next_id;
                        // A question with nothing to choose between: Hermes asks for it with a
                        // bare string and no values (`acp_adapter/elicitation.py`, where the
                        // schema is `{"type": "string"}` when no choices were offered).
                        request(
                            asked,
                            "elicitation/create",
                            json!({
                                "sessionId": session,
                                "mode": "form",
                                "message": "What should I call it?",
                                "requestedSchema": {
                                    "type": "object",
                                    "properties": {"answer": {"type": "string"}},
                                    "required": ["answer"]
                                }
                            }),
                        );
                        let written = wait_for_answer(&mut input, asked)
                            .and_then(|value| value.get("content").cloned())
                            .and_then(|content| content.get("answer").cloned())
                            .and_then(|answer| answer.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "none".to_owned());
                        chunk(&session, &format!("wrote: {written}"));
                        turn_answer(&session, id);
                    }
                    "choose-many" => {
                        next_id += 1;
                        let asked = next_id;
                        // The shape Hermes sends when several answers may be chosen: the
                        // same strings, as the items of an array.
                        request(
                            asked,
                            "elicitation/create",
                            json!({
                                "sessionId": session,
                                "mode": "form",
                                "message": "Which of these should I do?",
                                "requestedSchema": {
                                    "type": "object",
                                    "properties": {
                                        "answer": {
                                            "type": "array",
                                            "items": {"type": "string", "enum": [
                                                "Tests \u{2014} the whole suite",
                                                "Docs \u{2014} the reference page",
                                                "Release \u{2014} a version of its own"
                                            ]}
                                        }
                                    },
                                    "required": ["answer"]
                                }
                            }),
                        );
                        let picked = wait_for_answer(&mut input, asked)
                            .and_then(|value| value.get("content").cloned())
                            .and_then(|content| content.get("answer").cloned())
                            .and_then(|answer| answer.as_array().cloned())
                            .map(|answer| {
                                answer
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_else(|| "none".to_owned());
                        chunk(
                            &session,
                            &if picked.is_empty() {
                                "none".to_owned()
                            } else {
                                format!("chose: {picked}")
                            },
                        );
                        turn_answer(&session, id);
                    }
                    "choose" => {
                        next_id += 1;
                        let asked = next_id;
                        // How many answers to offer: three with their own names, or as many
                        // as a test asks for, so that a card can be made taller than the
                        // panel it is drawn in.
                        let count = std::env::var("ACP_TEST_VARIANTS")
                            .ok()
                            .and_then(|count| count.parse::<usize>().ok())
                            .unwrap_or(3);
                        let answers: Vec<Value> = (0..count)
                            .map(|index| match index {
                                0 => json!({"const": "compact", "title": "Compact \u{2014} one line, no detail"}),
                                1 => json!({"const": "readable", "title": "Readable \u{2014} a few lines, with the reasoning behind them"}),
                                2 => json!({"const": "formal", "title": "Formal \u{2014} a full report in the usual structure"}),
                                other => json!({
                                    "const": format!("answer-{other}"),
                                    "title": format!("Answer {other} \u{2014} a choice of its own, written out at length")
                                }),
                            })
                            .collect();
                        request(
                            asked,
                            "elicitation/create",
                            json!({
                                "sessionId": session,
                                "mode": "form",
                                "message": "Which variant should I write?",
                                "requestedSchema": {
                                    "type": "object",
                                    "properties": {
                                        "variant": {
                                            "type": "string",
                                            "oneOf": answers
                                        }
                                    },
                                    "required": ["variant"]
                                }
                            }),
                        );
                        let chosen = wait_for_answer(&mut input, asked)
                            .and_then(|value| value.get("content").cloned())
                            .and_then(|content| content.get("variant").cloned())
                            .and_then(|variant| variant.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "none".to_owned());
                        chunk(&session, &chosen);
                        turn_answer(&session, id);
                    }
                    "read" => {
                        next_id += 1;
                        let asked = next_id;
                        request(
                            asked,
                            "fs/read_text_file",
                            json!({"sessionId": session, "path": "a.txt"}),
                        );
                        let content = wait_for_answer(&mut input, asked)
                            .and_then(|value| value.get("content").cloned())
                            .and_then(|content| content.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "nothing".to_owned());
                        chunk(&session, &content);
                        turn_answer(&session, id);
                    }
                    "write" => {
                        next_id += 1;
                        let asked = next_id;
                        request(
                            asked,
                            "fs/write_text_file",
                            json!({
                                "sessionId": session,
                                "path": "written.txt",
                                "content": "from the agent\n"
                            }),
                        );
                        let written = wait_for_answer(&mut input, asked)
                            .map(|value| !value.is_null())
                            .unwrap_or(false);
                        chunk(&session, if written { "written" } else { "write refused" });
                        turn_answer(&session, id);
                    }
                    // The directory the client opened the session in, and what it is set to:
                    // so a test can see the hand-over and the switches arrive.
                    "where" => {
                        chunk(&session, &cwd);
                        turn_answer(&session, id);
                    }
                    // What the client said about files and terminals when it opened the
                    // session: an agent that is not told works on the wrong machine.
                    "caps" => {
                        chunk(&session, &caps);
                        turn_answer(&session, id);
                    }
                    // Which conversation this is, and how it was opened: a test can tell a
                    // resumed session from a fresh one without reading the agent's mind.
                    "session" => {
                        let how = if resumed { "loaded" } else { "new" };
                        chunk(&session, &format!("{session_name} {how}"));
                        turn_answer(&session, id);
                    }
                    "model" => {
                        chunk(&session, &format!("{model} / {policy}"));
                        turn_answer(&session, id);
                    }
                    "slow" => {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        turn_answer(&session, id);
                    }
                    "die" => {
                        std::process::exit(3);
                    }
                    other => {
                        chunk(&session, other);
                        turn_answer(&session, id);
                    }
                }
            }
            _ => {
                if let Some(id) = id {
                    send(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": "no such method"}
                    }));
                }
            }
        }
    }
}

/// One `session/update` notification carrying text.
fn chunk(session: &str, text: &str) {
    send(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": text}
            }
        }
    }));
}

/// One `session/update` notification carrying a piece of reasoning.
fn thought(session: &str, text: &str) {
    send(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session,
            "update": {
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": text}
            }
        }
    }));
}

/// A request of our own, answered by the client.
fn request(id: u64, method: &str, params: Value) {
    send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
}

/// The answer to request `id`, reading and ignoring whatever else arrives meanwhile.
fn wait_for_answer(input: &mut std::io::Lines<std::io::StdinLock<'_>>, id: u64) -> Option<Value> {
    for line in input.by_ref() {
        let Ok(line) = line else { return None };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) == Some(id) && message.get("method").is_none()
        {
            return Some(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }
    None
}

/// An answer to one of the client's requests.
fn answer(id: Option<u64>, result: Value) {
    let Some(id) = id else { return };
    send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

/// One line out.
fn send(message: Value) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{message}");
    let _ = out.flush();
}
