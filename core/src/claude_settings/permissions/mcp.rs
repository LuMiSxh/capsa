// SPDX-License-Identifier: MPL-2.0
//! The MCP rules: `mcp__<server>`, `mcp__<server>__*` and `mcp__<server>__<tool>`.

use super::Permissions;
use super::Refusal;
use super::Rule;
use super::Tool;
use super::policy::Scope;

impl Permissions {
    /// Refuses `mcp__<server>__<tool>`, or the whole server when `tool` is `None`, if a deny or
    /// ask rule covers it, or if the mode needs an allow rule and none covers it.
    pub fn check_mcp(&self, server: &str, tool: Option<&str>) -> Result<(), Refusal> {
        let server = claude_name(server);
        let whole = format!("mcp__{server}");
        let what = tool.map_or_else(
            || format!("the MCP server {server}"),
            |t| format!("{whole}__{t}"),
        );
        let entries = self.entries()?;
        let covers = |text: &str| {
            text == whole
                || text == format!("{whole}__*")
                || tool.is_some_and(|t| text == format!("{whole}__{t}"))
        };
        for rule in [Rule::Deny, Rule::Ask] {
            if let Some(entry) = entries.iter().find(|e| e.rule == rule && covers(&e.text)) {
                return Err(entry.refusal(&what));
            }
        }
        self.gate(Tool::Mcp, Scope::Elsewhere, &what, || {
            entries
                .iter()
                .any(|e| e.rule == Rule::Allow && covers(&e.text))
        })
    }
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
