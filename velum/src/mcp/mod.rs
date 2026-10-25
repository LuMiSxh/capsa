// SPDX-License-Identifier: MPL-2.0
//! Owns `velum mcp`: a stdio MCP server (JSON-RPC 2.0, one message per line) that forwards
//! tool calls to the approval broker named by `VELUM_BROKER`.

pub mod config;
#[cfg(test)]
mod tests;

use crate::broker::{ENV, Response, Status};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Protocol versions this server speaks, newest first.
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// The broker may wait two minutes for the person and then run a command for ten.
const BROKER_TIMEOUT: Duration = Duration::from_secs(13 * 60);

const NOT_UNDER_VELUM: &str =
    "not running under velum: VELUM_BROKER is not set, so there is no approval broker to ask";

/// The tools, each as `(name, description, input schema)`.
fn tools() -> Value {
    json!([
        {
            "name": "git_push",
            "description": "Push the current branch of the working directory to the \
                same-named branch of a remote, outside the sandbox. The user is asked first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "remote": {"type": "string", "description": "Remote name (default origin)."},
                    "force": {"type": "boolean", "description": "Use --force-with-lease."}
                }
            }
        },
        {
            "name": "gh",
            "description": "Run the GitHub CLI in the working directory, outside the sandbox. \
                The user is asked first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "args": {"type": "array", "items": {"type": "string"},
                             "description": "Arguments after `gh`."}
                },
                "required": ["args"]
            }
        },
        {
            "name": "open_url",
            "description": "Open an http or https URL in the user's browser. The user is \
                asked first.",
            "inputSchema": {
                "type": "object",
                "properties": {"url": {"type": "string"}},
                "required": ["url"]
            }
        },
        {
            "name": "run_outside",
            "description": "Run a shell command (zsh -lc) in the working directory, outside \
                the sandbox, with the user's credentials. The user is asked every time.",
            "inputSchema": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }
        }
    ])
}

/// Reads messages from `input` until it ends and writes the answers to `output`.
/// `socket` is the broker's socket, if there is one.
pub fn serve(input: impl BufRead, mut output: impl Write, socket: Option<&Path>) -> Result<()> {
    for line in input.lines() {
        let line = line.context("reading a message")?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&message, socket),
            Err(err) => Some(failure(
                &Value::Null,
                -32700,
                &format!("parse error: {err}"),
            )),
        };
        if let Some(answer) = answer {
            writeln!(output, "{answer}").context("writing an answer")?;
            output.flush().context("writing an answer")?;
        }
    }
    Ok(())
}

/// Returns the broker socket from `VELUM_BROKER`; unset and empty both mean none.
pub fn socket_from_env() -> Option<PathBuf> {
    std::env::var_os(ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn success(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn failure(id: &Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Answers one message. A notification (no `id`) gets no answer.
fn handle(message: &Value, socket: Option<&Path>) -> Option<Value> {
    let id = message.get("id")?;
    let params = message.get("params").unwrap_or(&Value::Null);
    let answer = match message.get("method").and_then(Value::as_str) {
        Some("initialize") => success(id, &initialize(params)),
        Some("ping") => success(id, &json!({})),
        Some("tools/list") => success(id, &json!({"tools": tools()})),
        Some("tools/call") => call(id, params, socket),
        Some(other) => failure(id, -32601, &format!("method not found: {other}")),
        None => failure(id, -32600, "invalid request"),
    };
    Some(answer)
}

fn initialize(params: &Value) -> Value {
    let wanted = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOLS
        .iter()
        .find(|supported| Some(**supported) == wanted)
        .unwrap_or(&PROTOCOLS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "velum", "version": env!("CARGO_PKG_VERSION")}
    })
}

fn call(id: &Value, params: &Value, socket: Option<&Path>) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let known = tools()
        .as_array()
        .is_some_and(|all| all.iter().any(|tool| tool["name"] == name));
    if !known {
        return failure(id, -32602, &format!("unknown tool: {name}"));
    }
    let Some(socket) = socket else {
        return success(id, &tool_result(true, NOT_UNDER_VELUM));
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let request = json!({"tool": name, "args": arguments});
    let result = match ask_broker(socket, &request) {
        Ok(response) => describe(&response),
        Err(err) => tool_result(
            true,
            &format!("the approval broker did not answer: {err:#}"),
        ),
    };
    success(id, &result)
}

fn tool_result(is_error: bool, text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

/// Turns the broker's answer into a tool result.
fn describe(response: &Response) -> Value {
    match response.status {
        Status::Done => {
            let code = response.exit_code.unwrap_or(1);
            let text = format!("exit code: {code}\n{}", response.output);
            tool_result(code != 0, &text)
        }
        Status::Denied => tool_result(true, &format!("denied: {}", response.message)),
        Status::Error => tool_result(true, &response.message),
    }
}

/// Sends one request line to the broker and reads one response line.
fn ask_broker(socket: &Path, request: &Value) -> Result<Response> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("connecting to {}", socket.display()))?;
    stream.set_read_timeout(Some(BROKER_TIMEOUT))?;
    writeln!(stream, "{request}").context("sending the request")?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .context("reading the answer")?;
    serde_json::from_str(&line).context("the answer is not valid")
}
