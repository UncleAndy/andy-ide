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
//! - `read` — asks the client for `a.txt`, and answers with what it was given.
//! - `write` — asks the client to write `written.txt`, and says whether it could.
//! - `slow` — waits five seconds before answering.
//! - `die` — leaves without answering, the way a crashed agent does.
//! - anything else — echoes the words back as a single chunk.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// Ends a turn, saying how much of the window it used: ACP has no field for this, so the
/// figure rides in `_meta`, the way a real agent would send it.
fn turn_answer(id: Option<u64>) {
    answer(
        id,
        json!({"stopReason": "end_turn", "_meta": {"usage": {"used": 42000, "window": 100000}}}),
    );
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
    let mut mode = "quick".to_owned();
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
                answer(
                    id,
                    json!({
                        "sessionId": session_name,
                        "models": {
                            "currentModelId": model,
                            "availableModels": [
                                {"modelId": "small", "name": "Small"},
                                {"modelId": "large", "name": "Large"}
                            ]
                        },
                        "modes": {
                            "currentModeId": mode,
                            "availableModes": [
                                {"id": "quick", "name": "Quick"},
                                {"id": "deep", "name": "Deep"}
                            ]
                        }
                    }),
                );
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
                            "availableModels": [
                                {"modelId": "small", "name": "Small"},
                                {"modelId": "large", "name": "Large"}
                            ]
                        },
                        "modes": {
                            "currentModeId": mode,
                            "availableModes": [
                                {"id": "quick", "name": "Quick"},
                                {"id": "deep", "name": "Deep"}
                            ]
                        }
                    }),
                );
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
                    mode = chosen.to_owned();
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
                        turn_answer(id);
                    }
                    "markdown" => {
                        chunk(&session, "**bold** and `code`");
                        turn_answer(id);
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
                        turn_answer(id);
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
                        turn_answer(id);
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
                        turn_answer(id);
                    }
                    // The directory the client opened the session in, and what it is set to:
                    // so a test can see the hand-over and the switches arrive.
                    "where" => {
                        chunk(&session, &cwd);
                        turn_answer(id);
                    }
                    // What the client said about files and terminals when it opened the
                    // session: an agent that is not told works on the wrong machine.
                    "caps" => {
                        chunk(&session, &caps);
                        turn_answer(id);
                    }
                    // Which conversation this is, and how it was opened: a test can tell a
                    // resumed session from a fresh one without reading the agent's mind.
                    "session" => {
                        let how = if resumed { "loaded" } else { "new" };
                        chunk(&session, &format!("{session_name} {how}"));
                        turn_answer(id);
                    }
                    "model" => {
                        chunk(&session, &format!("{model} / {mode}"));
                        turn_answer(id);
                    }
                    "slow" => {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        turn_answer(id);
                    }
                    "die" => {
                        std::process::exit(3);
                    }
                    other => {
                        chunk(&session, other);
                        turn_answer(id);
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
