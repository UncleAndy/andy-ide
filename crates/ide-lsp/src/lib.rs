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
