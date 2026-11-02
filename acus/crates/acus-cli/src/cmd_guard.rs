// SPDX-License-Identifier: MPL-2.0
//! The `guard` hook: reads one tool call from stdin and prints the hook's answer. The decisions
//! are in `acus_guard`; this file is the stdin/stdout protocol around them.

use crate::config::Config;
use acus_guard::{BashDecision, GuardConfig, Scope, Verdict};
use anyhow::Result;
use capsa_core::exit::Outcome;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct Args {
    /// Decide each Bash call in a JSONL file in-process and print the decisions; guard.jsonl is not written.
    #[arg(long, value_name = "FILE", hide = true)]
    replay: Option<PathBuf>,
    /// Decide each call this many times and print it once, so a benchmark can time decisions
    /// rather than process start-up.
    #[arg(long, default_value_t = 1, hide = true, requires = "replay")]
    repeat: usize,
}

/// The guard's files, next to the config so they follow `ACUS_CONFIG`.
pub fn guard_config(cfg: &Config) -> GuardConfig {
    GuardConfig {
        hint_every: cfg.guard.hint_every,
        state_dir: cfg.path.with_file_name("guard-hints"),
        log_path: cfg.path.with_file_name("guard.jsonl"),
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, cfg: &Config, config_problem: Option<&str>) -> Result<Outcome> {
    if let Some(file) = &a.replay {
        acus_guard::replay_file(file, a.repeat)?;
        return Ok(Outcome::Found);
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let v: Value = serde_json::from_str(&input).unwrap_or_default();
    let input = &v["tool_input"];
    let cwd = v["cwd"].as_str();
    let command = input["command"].as_str().unwrap_or_default();
    let scope = Scope::new(cwd);
    let gcfg = guard_config(cfg);
    let session = v["session_id"].as_str().unwrap_or_default();
    let note = config_problem.and_then(|p| config_note(&gcfg, session, p, &cfg.path));
    // Codex hooks (they carry a `turn_id`) cannot rewrite a call, only refuse it.
    let can_rewrite = v["tool_name"].as_str() == Some("Bash") && v.get("turn_id").is_none();
    let verdict = match v["tool_name"].as_str() {
        Some("Read") => acus_guard::read_check(input, cwd, scope.as_ref()),
        Some("Grep") => acus_guard::grep_tool(input, cwd),
        Some("Glob") => Some(acus_guard::glob_tool()),
        Some("PowerShell") => acus_guard::check_ps(command, scope.as_ref()),
        _ => match acus_guard::decide_bash(command, scope.as_ref(), can_rewrite) {
            BashDecision::Rewrite(rw) => {
                acus_guard::log::append(&gcfg, &v, &Verdict::Rewrite(rw.rule));
                let mut updated = input.clone();
                updated["command"] = rw.command.into();
                let mut hook = serde_json::json!({
                    "hookEventName": "PreToolUse",
                    "updatedInput": updated,
                    // No permissionDecisionReason: an allow reason would reach only the user, not the agent.
                    "additionalContext": with_note(rw.note, note.as_deref()),
                });
                // A build is left undecided, so Claude Code's permission flow decides on the rewritten command.
                if !rw.deferred {
                    hook["permissionDecision"] = "allow".into();
                }
                println!("{}", serde_json::json!({ "hookSpecificOutput": hook }));
                return Ok(Outcome::Found);
            }
            BashDecision::Verdict(verdict) => Some(verdict),
            BashDecision::Pass => None,
        },
    };
    // A pure hint repeats for every call of its rule, so `hints` shows only some of them. A call
    // whose hint is hidden passes silently and is not logged.
    let Some(verdict) = verdict.filter(|v| shown(&gcfg, session, v)) else {
        return Ok(note_only(note));
    };
    acus_guard::log::append(&gcfg, &v, &verdict);
    let fields = match verdict {
        Verdict::Deny(_, reason) => serde_json::json!({
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": with_note(reason, note.as_deref()),
        }),
        // The command runs as usual; only the agent's context gains the hint.
        Verdict::Hint(_, hint) => serde_json::json!({
            "hookEventName": "PreToolUse",
            "additionalContext": with_note(hint, note.as_deref()),
        }),
        Verdict::Escape(_) | Verdict::Rewrite(_) => return Ok(note_only(note)),
    };
    println!("{}", serde_json::json!({ "hookSpecificOutput": fields }));
    Ok(Outcome::Found)
}

/// Reports whether a verdict is printed: a pure hint may be hidden by the throttle.
fn shown(cfg: &GuardConfig, session: &str, verdict: &Verdict) -> bool {
    !matches!(verdict, Verdict::Hint(rule, _) if !acus_guard::hints::show(cfg, session, rule.as_str()))
}

/// Returns the note for a config that did not load. Only the first call of each session carries
/// it: the config rule is set so that no second call of the session shows it.
fn config_note(gcfg: &GuardConfig, session: &str, problem: &str, path: &Path) -> Option<String> {
    let once = GuardConfig {
        hint_every: usize::MAX,
        ..gcfg.clone()
    };
    acus_guard::hints::show(&once, session, "config").then(|| {
        let problem = problem.split_whitespace().collect::<Vec<_>>().join(" ");
        format!(
            "acus config error: {problem}; guard runs with defaults; fix {}",
            path.display()
        )
    })
}

/// Appends the config note, when there is one, to the text the agent reads.
fn with_note(text: String, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{text}\n{note}"),
        None => text,
    }
}

/// Prints the config note as the hook's only context, when there is one.
fn note_only(note: Option<String>) -> Outcome {
    if let Some(note) = note {
        println!(
            "{}",
            serde_json::json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": note,
            }})
        );
    }
    Outcome::Found
}
