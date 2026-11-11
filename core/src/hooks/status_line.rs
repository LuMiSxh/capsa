// SPDX-License-Identifier: MPL-2.0
//! Sets, removes and reads the `statusLine` command of a tool in a settings document.

use serde_json::{Value, json};

use crate::claude_settings::{Step, put_key};

/// The `statusLine` command a tool runs, and the claim that tells the tool's own commands from
/// other ones.
#[derive(Debug, Clone, Copy)]
pub struct StatusLineSpec {
    /// The command the statusLine runs.
    pub command: &'static str,
    /// Claims the commands of the tool, current and older.
    pub owns: fn(&str) -> bool,
}

/// What a settings document holds for a `StatusLineSpec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusLineState {
    /// The statusLine runs the spec's command.
    Installed,
    /// The statusLine runs another command of the tool: an older form or other arguments.
    Outdated { command: String },
    /// The statusLine runs a command the tool does not own, or has no command string.
    Foreign { command: Option<String> },
    /// The document has no statusLine.
    Missing,
}

/// Returns the state of the statusLine that `spec` describes in `v`.
pub(super) fn state(v: &Value, spec: &StatusLineSpec) -> StatusLineState {
    let Some(line) = v.get("statusLine") else {
        return StatusLineState::Missing;
    };
    let command = line.get("command").and_then(Value::as_str);
    match command {
        Some(c) if c == spec.command => StatusLineState::Installed,
        Some(c) if (spec.owns)(c) => StatusLineState::Outdated {
            command: c.to_owned(),
        },
        _ => StatusLineState::Foreign {
            command: command.map(str::to_owned),
        },
    }
}

/// Sets the statusLine to `spec.command`. An identical command gives `Step::Set`, and any other
/// statusLine is replaced.
pub(super) fn set(v: &mut Value, spec: &StatusLineSpec) -> Step {
    let step = match state(v, spec) {
        StatusLineState::Installed => return Step::Set,
        StatusLineState::Missing => Step::Install,
        _ => Step::Replace,
    };
    put_key(
        v,
        "statusLine",
        json!({"type": "command", "command": spec.command}),
    );
    step
}

/// Removes the statusLine when it runs the tool's command. Returns whether one was removed.
pub(super) fn remove(v: &mut Value, spec: &StatusLineSpec) -> bool {
    if !matches!(
        state(v, spec),
        StatusLineState::Installed | StatusLineState::Outdated { .. }
    ) {
        return false;
    }
    v.as_object_mut()
        .and_then(|root| root.remove("statusLine"))
        .is_some()
}
