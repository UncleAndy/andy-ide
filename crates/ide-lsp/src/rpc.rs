//! JSON-RPC 2.0 as LSP uses it: requests with an id, notifications without one,
//! and responses that carry either a result or an error.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC: &str = "2.0";

/// A request the client sends and waits for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: i64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    pub fn new(id: i64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC.to_owned(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// A message without an id: the client is not waiting for an answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC.to_owned(),
            method: method.into(),
            params,
        }
    }
}

/// The answer to a request, as it arrives from the server.
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<ErrorObject>,
}

impl Response {
    /// The id as a number, which is what this client hands out.
    pub fn numeric_id(&self) -> Option<i64> {
        match self.id.as_ref()? {
            Value::Number(number) => number.as_i64(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

impl std::fmt::Display for ErrorObject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} (code {})", self.message, self.code)
    }
}

/// Anything the server can send us.
#[derive(Debug, Clone)]
pub enum Incoming {
    Response(Response),
    /// The server asking us something or telling us something (diagnostics,
    /// progress, log messages). Requests from the server are answered with
    /// "method not found" by the client, see [`crate::client`].
    Notification {
        method: String,
        params: Option<Value>,
    },
    ServerRequest {
        id: Value,
        method: String,
        /// What the server asked, when it asked with parameters.
        params: Option<Value>,
    },
}

/// Sorts one parsed message into the three shapes above.
///
/// A message with `method` and `id` is a request from the server, with `method`
/// alone a notification, and with `id` alone a response.
pub fn classify(value: Value) -> Option<Incoming> {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let id = value.get("id").cloned();

    match (method, id) {
        (Some(method), Some(id)) => Some(Incoming::ServerRequest {
            id,
            method,
            params: value.get("params").cloned(),
        }),
        (Some(method), None) => Some(Incoming::Notification {
            method,
            params: value.get("params").cloned(),
        }),
        (None, Some(_)) => serde_json::from_value::<Response>(value)
            .ok()
            .map(Incoming::Response),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_has_the_protocol_version() {
        let request = Request::new(3, "textDocument/definition", None);
        let text = serde_json::to_string(&request).unwrap();
        assert!(text.contains("\"jsonrpc\":\"2.0\""));
        assert!(text.contains("\"id\":3"));
        assert!(!text.contains("params"), "no params means no field");
    }

    #[test]
    fn a_response_splits_into_result_and_error() {
        let ok = classify(json!({"jsonrpc": "2.0", "id": 1, "result": null})).unwrap();
        match ok {
            Incoming::Response(response) => {
                assert_eq!(response.numeric_id(), Some(1));
                assert!(response.error.is_none());
            }
            other => panic!("expected a response, got {other:?}"),
        }

        let failed = classify(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "error": {"code": -32601, "message": "method not found"}
        }))
        .unwrap();
        match failed {
            Incoming::Response(response) => {
                let error = response.error.expect("an error");
                assert_eq!(error.code, -32601);
                assert!(error.to_string().contains("method not found"));
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn a_notification_without_an_id_is_not_a_response() {
        let incoming = classify(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {"uri": "file:///a.rs", "diagnostics": []}
        }))
        .unwrap();
        match incoming {
            Incoming::Notification { method, params } => {
                assert_eq!(method, "textDocument/publishDiagnostics");
                assert!(params.is_some());
            }
            other => panic!("expected a notification, got {other:?}"),
        }
    }

    #[test]
    fn a_request_from_the_server_is_recognised() {
        let incoming = classify(json!({
            "jsonrpc": "2.0",
            "id": "abc",
            "method": "workspace/configuration",
            "params": {}
        }))
        .unwrap();
        assert!(matches!(incoming, Incoming::ServerRequest { .. }));
    }

    #[test]
    fn a_string_id_is_not_mistaken_for_a_number() {
        let response: Response = serde_json::from_value(json!({"id": "abc", "result": 1})).unwrap();
        assert_eq!(response.numeric_id(), None);
    }
}
