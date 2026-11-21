// SPDX-License-Identifier: MPL-2.0
//! One MCP server process and the requests made to it.

use crate::McpError;
use capsa_core::process::{Signal, own_group, signal_group};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Instant;

const PROTOCOL_VERSION: &str = "2025-06-18";

/// The command that starts a stdio server.
#[derive(Debug, Clone)]
pub struct StdioServer {
    pub command: String,
    pub args: Vec<String>,
    /// Variables added to the inherited environment.
    pub env: Vec<(String, String)>,
}

/// A tool a server offers.
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
}

/// A running server. Dropping the client kills the server's whole process group.
pub struct Client {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
    alive: Arc<AtomicBool>,
}

/// Ends a server's process group without the client, which a blocked request may hold.
#[derive(Clone)]
pub struct Killer {
    pgid: u32,
    alive: Arc<AtomicBool>,
}

impl Killer {
    /// Kills the server's process group unless it was killed already. The client reaps the child
    /// when it is killed or dropped.
    pub fn kill(&self) {
        if self.alive.swap(false, Ordering::SeqCst) {
            // The group may be gone already.
            let _ = signal_group(self.pgid, Signal::Kill);
        }
    }
}

impl Client {
    /// Starts the server and completes the `initialize` handshake before `deadline`.
    ///
    /// # Errors
    /// Fails when the command cannot start, or the server does not complete the handshake in time.
    pub fn connect(server: &StdioServer, deadline: Instant) -> Result<Self, McpError> {
        let mut command = Command::new(&server.command);
        command
            .args(&server.args)
            .envs(server.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        own_group(&mut command);
        let mut child = command.spawn().map_err(|source| McpError::Spawn {
            command: server.command.clone(),
            source,
        })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(McpError::Closed);
        };
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let mut client = Client {
            child,
            stdin,
            lines,
            next_id: 1,
            alive: Arc::new(AtomicBool::new(true)),
        };
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "acus", "version": env!("CARGO_PKG_VERSION")},
        });
        client.request("initialize", &params, deadline)?;
        client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(client)
    }

    /// Returns every tool the server offers, following its pagination.
    ///
    /// # Errors
    /// Fails when the server does not answer before `deadline` or answers with an error.
    pub fn list_tools(&mut self, deadline: Instant) -> Result<Vec<Tool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor = None;
        loop {
            let params = cursor.map_or_else(|| json!({}), |c| json!({"cursor": c}));
            let page = self.request("tools/list", &params, deadline)?;
            let list = page["tools"].as_array().ok_or_else(|| McpError::Protocol {
                method: "tools/list".to_owned(),
                detail: "no `tools` array".to_owned(),
            })?;
            for tool in list {
                if let Some(name) = tool["name"].as_str() {
                    tools.push(Tool {
                        name: name.to_owned(),
                        description: tool["description"].as_str().unwrap_or_default().to_owned(),
                    });
                }
            }
            match page["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => return Ok(tools),
            }
        }
    }

    /// Calls a tool and returns the raw `tools/call` result (`content`, `structuredContent`,
    /// `isError`).
    ///
    /// # Errors
    /// Fails when the server does not answer before `deadline` or answers with an error.
    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: &Value,
        deadline: Instant,
    ) -> Result<Value, McpError> {
        let params = json!({"name": name, "arguments": arguments});
        self.request("tools/call", &params, deadline)
    }

    /// Returns a handle that kills the server from another thread.
    #[must_use]
    pub fn killer(&self) -> Killer {
        Killer {
            pgid: self.child.id(),
            alive: Arc::clone(&self.alive),
        }
    }

    /// Ends the server and everything it started. A later request fails with `Closed`.
    pub fn kill(&mut self) {
        self.killer().kill();
        let _ = self.child.wait();
    }

    fn send(&mut self, message: &Value) -> Result<(), McpError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(McpError::Closed);
        }
        writeln!(self.stdin, "{message}")
            .and_then(|()| self.stdin.flush())
            .map_err(McpError::Write)
    }

    /// Sends a request and waits for its response; a server that is silent at `deadline` is killed.
    fn request(
        &mut self,
        method: &str,
        params: &Value,
        deadline: Instant,
    ) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    self.kill();
                    return Err(McpError::Timeout {
                        method: method.to_owned(),
                    });
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.kill();
                    return Err(McpError::Closed);
                }
            };
            let Ok(mut message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("method").is_some() {
                // A request from the server (sampling, roots, ping) has no answer here.
                if let Some(request_id) = message.get("id") {
                    let refusal = json!({"jsonrpc": "2.0", "id": request_id,
                        "error": {"code": -32601, "message": "acus does not serve this method"}});
                    self.send(&refusal)?;
                }
                continue;
            }
            if message.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(McpError::Rpc {
                    method: method.to_owned(),
                    code: error["code"].as_i64().unwrap_or(0),
                    message: error["message"].as_str().unwrap_or_default().to_owned(),
                });
            }
            return Ok(message["result"].take());
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.kill();
    }
}
