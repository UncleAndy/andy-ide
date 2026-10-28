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

fn main() {
    // One reader for the whole program: two `lines()` iterators over the same input
    // each keep a buffer of their own, and the answer to a request of ours can land in
    // the buffer nobody is looking at.
    let stdin = std::io::stdin();
    let mut input = stdin.lock().lines();
    let mut next_id = 100;
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
                answer(id, json!({"protocolVersion": 1, "agentCapabilities": {}}));
            }
            "session/new" => {
                answer(id, json!({"sessionId": "stand-in-session"}));
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
                        chunk(&session, "Hel");
                        chunk(&session, "lo");
                        answer(id, json!({"stopReason": "end_turn"}));
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
                        answer(id, json!({"stopReason": "end_turn"}));
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
                        answer(id, json!({"stopReason": "end_turn"}));
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
                        answer(id, json!({"stopReason": "end_turn"}));
                    }
                    "slow" => {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        answer(id, json!({"stopReason": "end_turn"}));
                    }
                    "die" => {
                        std::process::exit(3);
                    }
                    other => {
                        chunk(&session, other);
                        answer(id, json!({"stopReason": "end_turn"}));
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
