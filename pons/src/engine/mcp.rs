// SPDX-License-Identifier: MPL-2.0

//! Adds the MCP servers set with `--mcp-server` to the client's session requests.

use super::*;

impl Engine {
    /// Forwards a session request with the configured MCP servers added to its
    /// `mcpServers`; the line goes out unchanged when none was added.
    pub(super) fn add_mcp_servers(&self, mut msg: Value, raw: &[u8]) -> Vec<Action> {
        if self.cfg.mcp_servers.is_empty() {
            return vec![Action::ToAgent(raw.to_vec())];
        }
        let at = method(&msg).unwrap_or_default().to_owned();
        let mut notes = Vec::new();
        let added = inject_mcp_servers(&mut msg, &self.cfg.mcp_servers, &at, &mut notes);
        let line = if added {
            msg.to_string().into_bytes()
        } else {
            raw.to_vec()
        };
        let mut out: Vec<Action> = notes.into_iter().map(Action::Note).collect();
        out.push(Action::ToAgent(line));
        out
    }
}

/// Appends `servers` to `params.mcpServers` of a session request, creating the array
/// when it is missing. A name the client already sends keeps the client's entry.
/// Returns whether a server was added and pushes one note per server.
fn inject_mcp_servers(
    msg: &mut Value,
    servers: &[McpServer],
    at: &str,
    notes: &mut Vec<String>,
) -> bool {
    let Some(params) = msg.get_mut("params").and_then(Value::as_object_mut) else {
        return false;
    };
    let list = params.entry("mcpServers").or_insert_with(|| json!([]));
    if list.is_null() {
        *list = json!([]);
    }
    let Some(list) = list.as_array_mut() else {
        notes.push(format!(
            "{at}: mcpServers is not an array, --mcp-server not added"
        ));
        return false;
    };
    let mut added = false;
    for server in servers {
        let taken = list
            .iter()
            .any(|s| s.get("name").and_then(Value::as_str) == Some(server.name.as_str()));
        if taken {
            notes.push(format!(
                "{at}: the client already names MCP server {}, kept its entry (--mcp-server ignored)",
                server.name
            ));
            continue;
        }
        list.push(json!({
            "name": server.name,
            "command": server.command,
            "args": server.args,
            "env": []
        }));
        notes.push(format!(
            "{at}: added MCP server {} (--mcp-server)",
            server.name
        ));
        added = true;
    }
    added
}
