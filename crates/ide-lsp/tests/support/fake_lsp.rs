//! A stand-in language server for the tests: it speaks the protocol, remembers
//! what it was told, and answers three questions from the document it was given.
//!
//! It is a `[[bin]]` target (see `Cargo.toml`) so the integration test can start
//! it with `CARGO_BIN_EXE_fake-lsp`. It answers like a server with full document
//! sync would, and deliberately publishes a diagnostic for every open document so
//! the notification path is exercised too.

use std::io::{BufReader, Write};

/// The wire format, repeated here on purpose: the fixture must not depend on the
/// crate it is testing, or a broken framing would still "pass".
fn read_message(input: &mut impl std::io::BufRead) -> Option<serde_json::Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            length = rest.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0u8; length?];
    std::io::Read::read_exact(input, &mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn write_message(out: &mut impl Write, message: serde_json::Value) {
    let body = serde_json::to_vec(&message).unwrap();
    write!(out, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    out.write_all(&body).unwrap();
    out.flush().unwrap();
}

fn main() {
    let mut out = std::io::stdout();
    let mut documents: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut input = BufReader::new(std::io::stdin());

    while let Some(message) = read_message(&mut input) {
        let method = message
            .get("method")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned();
        let id = message.get("id").cloned();
        let params = message
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        match method.as_str() {
            "initialize" => {
                if let Some(id) = id {
                    write_message(
                        &mut out,
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "capabilities": {
                                    "definitionProvider": true,
                                    "referencesProvider": true,
                                    "documentSymbolProvider": true,
                                    "hoverProvider": true,
                                    "textDocumentSync": 1
                                },
                                "serverInfo": {"name": "fake-lsp", "version": "0.0.1"}
                            }
                        }),
                    );
                }
            }
            "textDocument/didOpen" => {
                let document = &params["textDocument"];
                let uri = document["uri"].as_str().unwrap_or_default().to_owned();
                documents.insert(
                    uri.clone(),
                    document["text"].as_str().unwrap_or_default().to_owned(),
                );
                // A diagnostic on line 0, so the notification path is used.
                write_message(
                    &mut out,
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": {
                            "uri": uri,
                            "diagnostics": [{
                                "range": {
                                    "start": {"line": 0, "character": 0},
                                    "end": {"line": 0, "character": 1}
                                },
                                "severity": 2,
                                "source": "fake-lsp",
                                "message": "the fixture noticed this line"
                            }]
                        }
                    }),
                );
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if let Some(changes) = params["contentChanges"].as_array() {
                    for change in changes {
                        if let Some(text) = change["text"].as_str() {
                            documents.insert(uri.clone(), text.to_owned());
                        }
                    }
                }
            }
            "textDocument/definition" => {
                let uri = params["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let text = documents.get(&uri).cloned().unwrap_or_default();
                // Answers with the first line of the document, so a test can see
                // that didOpen actually arrived.
                let heading = text.lines().next().unwrap_or_default().to_owned();
                write_message(
                    &mut out,
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "uri": uri,
                            "range": {
                                "start": {"line": 0, "character": 0},
                                "end": {"line": 0, "character": heading.chars().count()}
                            }
                        }
                    }),
                );
            }
            "textDocument/hover" => {
                let text = params["textDocument"]["uri"]
                    .as_str()
                    .and_then(|uri| documents.get(uri))
                    .cloned()
                    .unwrap_or_default();
                write_message(
                    &mut out,
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "contents": {"kind": "plaintext", "value": format!("{} bytes", text.len())}
                        }
                    }),
                );
            }
            "textDocument/references" => {
                write_message(
                    &mut out,
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": []}),
                );
            }
            "textDocument/documentHighlight" => {
                // With `--ask` on the command line, this server behaves like one that needs a
                // setting before it can answer: it asks the client and *waits*. A client
                // that builds an answer and forgets to send it leaves this server
                // waiting forever, which is exactly the bug this fixture exists to catch.
                let mut setting = serde_json::Value::Null;
                if std::env::args().any(|arg| arg == "--ask") {
                    write_message(
                        &mut out,
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 9001,
                            "method": "workspace/configuration",
                            "params": {"items": [{"section": "fake"}]}
                        }),
                    );
                    loop {
                        let Some(answer) = read_message(&mut input) else {
                            return;
                        };
                        if answer.get("id").and_then(|id| id.as_i64()) == Some(9001)
                            && answer.get("method").is_none()
                        {
                            setting = answer
                                .get("result")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            break;
                        }
                    }
                }

                let uri = params["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let text = documents.get(&uri).cloned().unwrap_or_default();
                // One range per `main`, so a test can count what came back.
                let mut highlights = Vec::new();
                for (number, line) in text.lines().enumerate() {
                    if let Some(at) = line.find("main") {
                        highlights.push(serde_json::json!({
                            "range": {
                                "start": {"line": number, "character": at},
                                "end": {"line": number, "character": at + 4}
                            },
                            "kind": 1
                        }));
                    }
                }
                let mut result = serde_json::Value::Array(highlights);
                if setting.is_array() {
                    // The setting came back, so it is worth saying so in the answer.
                    result.as_array_mut().unwrap().push(serde_json::json!({
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 0}
                        },
                        "kind": 3
                    }));
                }
                write_message(
                    &mut out,
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}),
                );
            }
            "shutdown" => {
                write_message(
                    &mut out,
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": null}),
                );
            }
            "exit" => break,
            _ => {
                if let Some(id) = id {
                    write_message(
                        &mut out,
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32601, "message": format!("no {method}")}
                        }),
                    );
                }
            }
        }
    }
}
