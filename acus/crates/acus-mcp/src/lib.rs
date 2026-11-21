// SPDX-License-Identifier: MPL-2.0
//! Speaks the Model Context Protocol to a server it starts on stdio: newline-delimited JSON-RPC 2.0
//! with `initialize`, `tools/list` and `tools/call`. It knows nothing about where a server is
//! configured.

mod client;
mod error;

pub use client::{Client, Killer, StdioServer, Tool};
pub use error::McpError;
