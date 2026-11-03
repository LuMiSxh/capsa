// SPDX-License-Identifier: MPL-2.0
//! `vicarius hook session-start`: the SessionStart hook. It prints the DeepSeek agent context line
//! when the gate is open and an agent file is ready. In every other case it prints nothing, and it
//! always exits 0.

use std::io::Write;
use std::time::Duration;

use capsa_core::exit::Outcome;
use serde_json::json;
use vicarius::config::Config;

use crate::status_client::{self, ServiceStatus};

/// The time the hook waits for the service to accept the connection and answer.
const TIMEOUT: Duration = Duration::from_millis(300);

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Print the DeepSeek agent context line for a new session, when agents are available.
    SessionStart,
}

/// Runs the hook. Nothing it finds is an error, so the result is always `Found`.
pub fn run(a: &Args) -> Outcome {
    let Action::SessionStart = a.action;
    if let Some(line) = context_line() {
        let out = json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": line}});
        let _ = writeln!(std::io::stdout(), "{out}");
    }
    Outcome::Found
}

fn context_line() -> Option<String> {
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
        "vicarius: DeepSeek agents available{balance}: {}. Start with the Claude agents; ask Luca before using a -ds agent and report its cost afterwards (vicarius status). Do not pass a model parameter when spawning one.",
        entries.join(", ")
    ))
}
