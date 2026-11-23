// SPDX-License-Identifier: MPL-2.0
//! The host function `run()`: runs a shell command under the script's time limit, once the Claude
//! Code `Bash(...)` rules and the acus guard allow it.

use super::super::argv::Call;
use super::super::shapes;
use super::Bindings;
use super::args::{internal, parse};
use super::host_error;
use crate::Cmd;
use crate::cmd_run;
use crate::out::Out;
use acus_guard::{BashDecision, Scope, Verdict};
use acus_script::HostError;
use capsa_core::format::Format;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Seconds a `run()` command may take when the script gives no `timeout`: the host call is not
/// bounded by the script's deadline.
const RUN_TIMEOUT: u64 = 120;

impl Bindings {
    pub(super) fn run(&self, mut call: Call, deadline: Instant) -> Result<Value, HostError> {
        let [command] = call.positionals.as_slice() else {
            return Err(HostError::new("run() takes one command string")
                .with_hint("pass the command, then options: run('cargo test', {tail: 40})"));
        };
        self.guard(command)?;
        let json = call.flag("json");
        if json {
            call.options.insert("raw".to_owned(), json!(true));
            call.options
                .insert("maxOutput".to_owned(), json!(1_000_000));
        }
        clamp_timeout(
            &mut call.options,
            deadline.saturating_duration_since(Instant::now()),
        );
        let Cmd::Run(a) = parse("run", &call, &["json"])? else {
            return Err(internal("run"));
        };
        let report =
            cmd_run::compute(&a, &Out::new(Format::Json), &self.cfg).map_err(|e| host_error(&e))?;
        let mut value = shapes::run(&report)?;
        if json {
            let parsed = serde_json::from_str(value["text"].as_str().unwrap_or_default());
            value["json"] = parsed.map_err(|e| {
                HostError::new(format!("the command's output is not JSON: {e}")).with_hint(
                    "drop {json: true}, or print JSON only; stderr is merged into the output",
                )
            })?;
        }
        Ok(value)
    }

    /// Applies Claude Code's `Bash(...)` rules, then the decision the PreToolUse hook would give a
    /// Bash call; a refusal throws with the rule or the guard's reason as the hint. Nothing is
    /// rewritten, so a command the guard would fold into an acus call is refused and names it.
    pub(super) fn guard(&self, command: &str) -> Result<(), HostError> {
        self.permissions
            .check_bash(command)
            .map_err(super::super::refused)?;
        // `Scope` caches lazily without locking, so each call makes its own.
        let scope = Scope::new(self.cwd.to_str());
        match acus_guard::decide_bash(command, scope.as_ref(), false) {
            BashDecision::Verdict(Verdict::Deny(_, reason)) => Err(HostError::new(
                "run() refused the command: the acus guard blocks it",
            )
            .with_hint(reason)),
            _ => Ok(()),
        }
    }
}

/// Sets the command's `timeout` to the script's own or the default, cut to the whole seconds left
/// before the script's deadline (at least 1), so `run` kills the command's process group itself
/// before the script dies and leaves it behind.
pub(super) fn clamp_timeout(options: &mut serde_json::Map<String, Value>, left: Duration) {
    let left = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    let Some(asked) = options
        .get("timeout")
        .map_or(Some(RUN_TIMEOUT), Value::as_u64)
    else {
        return;
    };
    options.insert("timeout".to_owned(), json!(asked.min(left.max(1))));
}
