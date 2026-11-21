// SPDX-License-Identifier: MPL-2.0
//! Reads the MCP rules of Claude Code's `permissions.deny` and `permissions.ask`: a script cannot
//! prompt, so a tool either would be denied or would need an answer, and both are refused.

use acus_script::HostError;
use capsa_core::claude_settings;
use std::path::{Path, PathBuf};

/// A deny or ask rule that names an MCP server or tool.
pub(super) struct Rule {
    entry: String,
    ask: bool,
    file: PathBuf,
}

/// Returns the MCP rules of the user, project and local settings. A settings file that does not
/// parse is an error, because ignoring it could let a denied tool run.
pub(super) fn load(root: &Path) -> Result<Vec<Rule>, HostError> {
    let files = [
        managed_path(),
        claude_settings::path(),
        root.join(".claude/settings.json"),
        root.join(".claude/settings.local.json"),
    ];
    let mut rules = Vec::new();
    for file in files {
        let settings = claude_settings::read(&file)
            .map_err(|e| HostError::new(format!("cannot read {}: {e}", file.display())))?;
        for (key, ask) in [("deny", false), ("ask", true)] {
            let entries = settings.value["permissions"][key].as_array();
            for entry in entries.into_iter().flatten().filter_map(|e| e.as_str()) {
                if entry.starts_with("mcp__") {
                    rules.push(Rule {
                        entry: entry.to_owned(),
                        ask,
                        file: file.clone(),
                    });
                }
            }
        }
    }
    Ok(rules)
}

/// Returns where the organization's managed settings live. Debug builds, which the tests use, read
/// `ACUS_MANAGED_SETTINGS` instead; a release build has no override, so an agent cannot move the
/// file it must obey.
fn managed_path() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("ACUS_MANAGED_SETTINGS") {
        return PathBuf::from(path);
    }
    let path = if cfg!(target_os = "macos") {
        "/Library/Application Support/ClaudeCode/managed-settings.json"
    } else if cfg!(windows) {
        r"C:\Program Files\ClaudeCode\managed-settings.json"
    } else {
        "/etc/claude-code/managed-settings.json"
    };
    PathBuf::from(path)
}

/// Returns the error for `mcp__<server>__<tool>`, or for the whole server when `tool` is `None`,
/// if a rule covers it.
pub(super) fn refuse(rules: &[Rule], server: &str, tool: Option<&str>) -> Result<(), HostError> {
    let server = claude_name(server);
    let whole = format!("mcp__{server}");
    let rule = rules.iter().find(|r| {
        r.entry == whole
            || r.entry == format!("{whole}__*")
            || tool.is_some_and(|t| r.entry == format!("{whole}__{t}"))
    });
    let Some(rule) = rule else { return Ok(()) };
    let what = tool.map_or_else(
        || format!("the MCP server {server}"),
        |t| format!("{whole}__{t}"),
    );
    let (why, fix) = if rule.ask {
        ("needs approval", "ask")
    } else {
        ("is denied", "deny")
    };
    Err(HostError::new(format!(
        "{what} {why} by `{}` in permissions.{fix} ({})",
        rule.entry,
        rule.file.display()
    ))
    .with_hint("a script cannot prompt; call the tool from the conversation, or remove the rule"))
}

/// Returns the server name as it appears in `mcp__<server>__<tool>`: Claude Code replaces every
/// character outside letters, digits, `_` and `-` with `_`.
fn claude_name(server: &str) -> String {
    server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
