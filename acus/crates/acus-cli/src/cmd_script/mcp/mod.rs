// SPDX-License-Identifier: MPL-2.0
//! MCP servers from Claude Code's configuration as script modules: `import gh from "mcp:github"`
//! gives an object whose methods are the server's tools. A server is reachable only when the acus
//! config lists it under `[script] mcp`, and the Claude permission rules apply to its tools.

mod discover;

use acus_mcp::{Client, Killer, McpError, Tool};
use acus_script::HostError;
use capsa_core::claude_settings::permissions::Permissions;
use discover::Definition;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

/// The global function a generated module's methods call.
pub(super) const CALL: &str = "__mcp";

/// One server per name, started on first import and killed when this value is dropped or shut
/// down. A server handles one request at a time; calls to different servers run in parallel.
pub(super) struct Mcp {
    allowed: Vec<String>,
    config: PathBuf,
    cwd: PathBuf,
    root: PathBuf,
    deadline: Instant,
    permissions: Arc<Permissions>,
    servers: Mutex<HashMap<String, Server>>,
}

/// A running server, and the handle that kills it while a request holds the client.
#[derive(Clone)]
struct Server {
    client: Arc<Mutex<Client>>,
    killer: Killer,
}

impl Mcp {
    /// Reads nothing yet: files are read when a script first imports a server.
    pub(super) fn new(
        allowed: Vec<String>,
        config: PathBuf,
        cwd: PathBuf,
        deadline: Instant,
        permissions: Arc<Permissions>,
    ) -> Self {
        let root = permissions.root().to_path_buf();
        Mcp {
            allowed,
            config,
            cwd,
            root,
            deadline,
            permissions,
            servers: Mutex::new(HashMap::new()),
        }
    }

    /// Starts the server `name` if it is not running and returns the source of its module.
    pub(super) fn module(&self, name: &str) -> Result<String, HostError> {
        self.check_allowed(name)?;
        let definition = discover::find(name, &self.cwd, &self.root)?.ok_or_else(|| {
            HostError::new(format!(
                "no MCP server \"{name}\" in the Claude Code settings"
            ))
            .with_hint(format!(
                "define it in {} or {}",
                discover::claude_json().display(),
                self.root.join(".mcp.json").display()
            ))
        })?;
        let spec = match definition {
            Definition::Stdio(spec) => spec,
            Definition::Remote(kind) => {
                return Err(HostError::new(format!(
                    "the MCP server \"{name}\" uses the {kind} transport, which acus script does not support yet"
                ))
                .with_hint("only stdio servers import; reach this one through its own CLI with run()"));
            }
        };
        self.check_rules(name, None)?;
        let mut servers = self.servers.lock().unwrap_or_else(PoisonError::into_inner);
        let server = match servers.get(name) {
            Some(server) => server.clone(),
            None => {
                let client = Client::connect(&spec, self.deadline).map_err(|e| failure(&e))?;
                let server = Server {
                    killer: client.killer(),
                    client: Arc::new(Mutex::new(client)),
                };
                servers.insert(name.to_owned(), server.clone());
                server
            }
        };
        drop(servers);
        let tools = server
            .client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .list_tools(self.deadline)
            .map_err(|e| failure(&e))?;
        Ok(module_source(name, &tools))
    }

    /// Calls `[server, tool, arguments]`, as a generated module's methods do.
    pub(super) fn call(&self, args: &[Value]) -> Result<Value, HostError> {
        let [Value::String(server), Value::String(tool), arguments] = args else {
            return Err(
                HostError::new("an MCP call takes a server, a tool and arguments")
                    .with_hint("import the server and call its methods: gh.create_issue({title})"),
            );
        };
        self.check_allowed(server)?;
        self.check_rules(server, Some(tool))?;
        let server = self
            .servers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(server.as_str())
            .cloned()
            .ok_or_else(|| {
                HostError::new(format!("the MCP server \"{server}\" is not running"))
                    .with_hint(format!("import it first: import s from \"mcp:{server}\""))
            })?;
        let result = server
            .client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .call_tool(tool, arguments, self.deadline)
            .map_err(|e| failure(&e))?;
        tool_value(tool, result)
    }

    /// Kills every server started so far. It does not wait for a request in flight: a worker
    /// blocked on a server that never reads would hold the client past the deadline.
    pub(super) fn shutdown(&self) {
        let servers =
            std::mem::take(&mut *self.servers.lock().unwrap_or_else(PoisonError::into_inner));
        for server in servers.into_values() {
            server.killer.kill();
        }
    }

    /// Returns the source of a module for `mcp:NAME` that starts nothing, for checking a script
    /// that will be saved. The opt-in still applies.
    pub(super) fn stub(&self, name: &str) -> Result<String, HostError> {
        self.check_allowed(name)?;
        Ok(module_source(name, &[]))
    }

    fn check_allowed(&self, name: &str) -> Result<(), HostError> {
        if self.allowed.iter().any(|a| a == name) {
            return Ok(());
        }
        Err(HostError::new(format!(
            "the MCP server \"{name}\" is not enabled for scripts"
        ))
        .with_hint(format!(
            "list it in {} as `[script]` `mcp = [\"{name}\"]`",
            self.config.display()
        )))
    }

    fn check_rules(&self, server: &str, tool: Option<&str>) -> Result<(), HostError> {
        self.permissions
            .check_mcp(server, tool)
            .map_err(super::refused)
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Returns the module whose default export has one async method per tool, and whose `tools`
/// export lists their names and descriptions.
fn module_source(server: &str, tools: &[Tool]) -> String {
    let list: Vec<Value> = tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description}))
        .collect();
    format!(
        "export const tools = {list};\nconst api = Object.create(null);\n\
         for (const {{ name }} of tools) api[name] = (args) => {CALL}({server}, name, args ?? {{}});\n\
         export default api;\n",
        list = Value::Array(list),
        server = Value::String(server.to_owned()),
    )
}

/// Returns what a script sees of a `tools/call` result: `structuredContent`, else the text content
/// joined, else the raw content. A tool error throws with its text.
fn tool_value(tool: &str, mut result: Value) -> Result<Value, HostError> {
    let content = result["content"].as_array().cloned().unwrap_or_default();
    let texts: Option<Vec<&str>> = content
        .iter()
        .map(|part| {
            (part["type"] == "text")
                .then(|| part["text"].as_str())
                .flatten()
        })
        .collect();
    if result["isError"].as_bool() == Some(true) {
        let message = texts.map(|t| t.join("\n")).filter(|t| !t.is_empty());
        return Err(HostError::new(
            message.unwrap_or_else(|| format!("the tool {tool} failed")),
        ));
    }
    let structured = result["structuredContent"].take();
    if !structured.is_null() {
        return Ok(structured);
    }
    Ok(match texts {
        Some(texts) if !content.is_empty() => Value::String(texts.join("\n")),
        _ => Value::Array(content),
    })
}

fn failure(error: &McpError) -> HostError {
    let failure = HostError::new(error.to_string());
    match error {
        McpError::Timeout { .. } => failure
            .with_hint("narrow the call, or raise `timeout` under `[script]` in the acus config"),
        McpError::Spawn { .. } => failure.with_hint("check `command` in the server's definition"),
        _ => failure,
    }
}
