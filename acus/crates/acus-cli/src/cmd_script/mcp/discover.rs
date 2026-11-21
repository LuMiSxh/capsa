// SPDX-License-Identifier: MPL-2.0
//! Finds an MCP server's definition the way Claude Code does: the local scope (`projects[<dir>]`
//! in `~/.claude.json`), then the project's `.mcp.json`, then the user scope (`mcpServers` in
//! `~/.claude.json`). Only the keys a start needs are read, and no value is ever printed.

use acus_mcp::StdioServer;
use acus_script::HostError;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A server's definition.
pub(super) enum Definition {
    Stdio(StdioServer),
    /// A transport other than stdio, named as the config names it.
    Remote(String),
}

/// Returns the definition of `name`, or `None` when no scope defines it.
pub(super) fn find(name: &str, cwd: &Path, root: &Path) -> Result<Option<Definition>, HostError> {
    let claude = read_json(&claude_json())?;
    let from_claude = |value: &Value| {
        value["mcpServers"][name]
            .as_object()
            .map(|_| value["mcpServers"][name].clone())
    };
    let local = cwd
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
        .find_map(|dir| {
            let key = dir.to_string_lossy();
            claude
                .as_ref()
                .and_then(|c| from_claude(&c["projects"][key.as_ref()]))
        });
    let project = || -> Result<Option<Value>, HostError> {
        Ok(read_json(&root.join(".mcp.json"))?.and_then(|f| from_claude(&f)))
    };
    let spec = match local {
        Some(spec) => Some(spec),
        None => match project()? {
            Some(spec) => Some(spec),
            None => claude.as_ref().and_then(from_claude),
        },
    };
    spec.map(|spec| definition(name, &spec)).transpose()
}

/// Returns `~/.claude.json`, or `$CLAUDE_CONFIG_DIR/.claude.json` when that is set.
pub(super) fn claude_json() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => std::env::home_dir()
            .unwrap_or_default()
            .join(".claude.json"),
    }
}

fn read_json(path: &Path) -> Result<Option<Value>, HostError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(HostError::new(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| HostError::new(format!("{} is not valid JSON: {e}", path.display())))
}

fn definition(name: &str, spec: &Value) -> Result<Definition, HostError> {
    let kind = spec["type"].as_str().unwrap_or("stdio");
    if kind != "stdio" {
        return Ok(Definition::Remote(kind.to_owned()));
    }
    let command = spec["command"].as_str().ok_or_else(|| {
        HostError::new(format!("the MCP server \"{name}\" has no `command`"))
            .with_hint("a stdio server needs `command` (and optionally `args` and `env`)")
    })?;
    let args = spec["args"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(expand).collect())
        .unwrap_or_default();
    let env = spec["env"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), expand(v.as_str()?))))
                .collect()
        })
        .unwrap_or_default();
    Ok(Definition::Stdio(StdioServer {
        command: expand(command),
        args,
        env,
    }))
}

/// Replaces `${VAR}` and `${VAR:-default}` with the environment, as Claude Code does.
fn expand(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            rest = &rest[start..];
            break;
        };
        let inner = &rest[start + 2..start + end];
        let (var, default) = inner.split_once(":-").unwrap_or((inner, ""));
        out.push_str(&std::env::var(var).unwrap_or_else(|_| default.to_owned()));
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}
