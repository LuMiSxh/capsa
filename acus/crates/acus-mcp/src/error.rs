// SPDX-License-Identifier: MPL-2.0
//! Why a request to an MCP server failed.

/// A failed start or request, one variant per way the caller should explain it.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// The server's command could not start.
    #[error("cannot start `{command}`: {source}")]
    Spawn {
        command: String,
        source: std::io::Error,
    },
    /// The request could not be written to the server.
    #[error("cannot write to the server: {0}")]
    Write(std::io::Error),
    /// The server ended, or was ended after an earlier failure.
    #[error("the server is not running")]
    Closed,
    /// The server did not answer before the deadline; it has been killed.
    #[error("the server did not answer `{method}` in time and was killed")]
    Timeout { method: String },
    /// The server answered with a JSON-RPC error.
    #[error("the server refused `{method}`: {message} (code {code})")]
    Rpc {
        method: String,
        code: i64,
        message: String,
    },
    /// The server's answer does not have the shape the protocol defines.
    #[error("the server's answer to `{method}` is malformed: {detail}")]
    Protocol { method: String, detail: String },
}
