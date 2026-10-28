//! LSP client and language server registry.
//!
//! This crate is UI-agnostic: it must never depend on `egui` or `eframe`.
//!
//! The client is deliberately synchronous with threads rather than `tokio` and
//! `async-lsp`: the application is a frame loop that must never block, and a
//! reader thread plus a request timeout gives that without an async runtime in
//! the core. The wire types come from `lsp-types`, so nothing about the protocol
//! is invented here.

pub mod client;
pub mod framing;
pub mod position;
pub mod registry;
pub mod rpc;
pub mod uri;

pub use client::{INITIALIZE_TIMEOUT, LspSession, REQUEST_TIMEOUT, ServerProcess};
pub use position::{byte_to_position, position_to_byte};
pub use registry::{SERVERS, ServerConfig, is_installed, server_for};
pub use uri::{path_to_uri, uri_to_path};

/// Version of the crate, taken from its manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Whether the server refused because its copy of the project was out of date.
///
/// `ContentModified` (-32801) is what a server says while it is still loading:
/// the question is about content it has not caught up with, so asking again in a
/// moment is right, unlike a protocol error which would only repeat itself. The
/// code is in the message because the request path carries the protocol error as
/// text; matching on the number is what keeps this from depending on wording.
pub fn is_content_modified(error: &anyhow::Error) -> bool {
    format!("{error:#}").contains("code -32801")
}

#[cfg(test)]
mod retry_tests {
    use super::is_content_modified;

    fn protocol_error(code: i64, message: &str) -> anyhow::Error {
        let object = crate::rpc::ErrorObject {
            code,
            message: message.to_owned(),
            data: None,
        };
        anyhow::anyhow!("{object}")
    }

    #[test]
    fn content_modified_is_worth_asking_again() {
        assert!(is_content_modified(&protocol_error(
            -32801,
            "content modified"
        )));
    }

    #[test]
    fn other_errors_are_not() {
        assert!(!is_content_modified(&protocol_error(
            -32601,
            "method not found"
        )));
        assert!(!is_content_modified(&anyhow::anyhow!("server died")));
    }
}
