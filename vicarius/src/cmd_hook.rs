// SPDX-License-Identifier: MPL-2.0
//! `vicarius hook`: the Claude Code hook. It dispatches on the input's `hook_event_name` and, on
//! `SessionStart`, prints the DeepSeek agent context line when the gate is open and an agent file
//! is ready. Every other event, every invalid input and every failure print nothing, and it always
//! exits 0.

use std::io::{IsTerminal, Read, Write};
use std::time::Duration;

use capsa_core::exit::Outcome;
use serde::Deserialize;
use serde_json::json;
use vicarius::config::Config;

use crate::status_client::{self, ServiceStatus};

/// The time the hook waits for the service to accept the connection and answer.
const TIMEOUT: Duration = Duration::from_millis(300);
/// The most hook input read; a larger one is not a hook input.
const MAX_INPUT: u64 = 1 << 20;

/// The fields of the hook input that vicarius reads.
#[derive(Deserialize)]
struct Input {
    hook_event_name: Option<String>,
}

/// Runs the hook. Nothing it finds is an error, so the result is always `Found`.
pub fn run() -> Outcome {
    if let Some(line) = read_input().as_deref().and_then(context_line) {
        let out = json!({
            "hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": line}
        });
        let _ = writeln!(std::io::stdout(), "{out}");
    }
    Outcome::Found
}

/// Reads stdin up to the cap; `None` for a terminal, a larger input or text that is not UTF-8.
fn read_input() -> Option<String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut buf = Vec::new();
    stdin
        .lock()
        .take(MAX_INPUT + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_INPUT {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// Returns the context line for a `SessionStart` input; `None` for another event or any failure.
fn context_line(input: &str) -> Option<String> {
    let input: Input = serde_json::from_str(input).ok()?;
    if input.hook_event_name.as_deref() != Some("SessionStart") {
        return None;
    }
    let config = Config::load_from(&Config::path()).ok()?;
    let fetched = status_client::fetch(config.port, TIMEOUT)?;
    agents_line(&config, &fetched.status)
}

/// Builds the context line from the status and the configured models, or `None` when the gate is
/// closed or no agent file is ready.
fn agents_line(config: &Config, status: &ServiceStatus) -> Option<String> {
    if !status.gate.open {
        return None;
    }
    let entries: Vec<String> = status
        .agents
        .as_ref()?
        .entries
        .iter()
        .filter(|entry| matches!(entry.outcome_name(), "written" | "unchanged"))
        .filter_map(|entry| {
            let agent = &entry.agent;
            let model = &config.agents.iter().find(|a| &a.from == agent)?.model;
            Some(format!("{agent}-ds ({model}, variant of {agent})"))
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    let balance = status.gate.balance.map_or_else(String::new, |amount| {
        let money = status_client::money(amount, status.gate.currency.as_deref());
        format!(" (balance {money})")
    });
    Some(format!(
        "vicarius: DeepSeek agents available{balance}: {}. Prefer them over their Claude source agents for delegated work and report the cost afterwards (vicarius status). Do not pass a model parameter when spawning one.",
        entries.join(", ")
    ))
}
