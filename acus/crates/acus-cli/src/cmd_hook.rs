// SPDX-License-Identifier: MPL-2.0
//! `acus hook`: reads one hook event from stdin. `SessionStart` and `SubagentStart` print the
//! core skill; `PreToolUse`, and a Codex payload without an event, runs the guard; another event
//! does nothing.

use crate::config::Config;
use crate::guard_log;
use acus_guard::{BashDecision, GuardConfig, Rewrite, Scope, Verdict};
use anyhow::Result;
use capsa_core::exit::Outcome;
use serde_json::Value;
use std::borrow::Cow;
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

/// The guard's files: the hints next to the config, so they follow `ACUS_CONFIG`, and the log in
/// the state directory.
pub fn guard_config(cfg: &Config) -> GuardConfig {
    GuardConfig {
        hint_every: cfg.guard.hint_every,
        hint_min: cfg.guard.hint_every_min(),
        hint_max: cfg.guard.hint_every_max(),
        state_dir: cfg.path.with_file_name("guard-hints"),
        log_path: guard_log::path(&cfg.path),
    }
}

pub fn run(a: &Args, cfg: &Config, config_problem: Option<&str>) -> Result<Outcome> {
    if let Some(file) = &a.replay {
        acus_guard::replay_file(file, a.repeat)?;
        return Ok(Outcome::Found);
    }
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    match v["hook_event_name"].as_str() {
        Some("SessionStart") => session_start(&v, cfg),
        Some("SubagentStart") => subagent_start(),
        Some("PreToolUse") | None => answer(v, cfg, config_problem),
        Some(_) => Ok(Outcome::Found),
    }
}

/// Prints the core skill for a SessionStart, forgetting the hint counts after a compaction or
/// `/clear`, and registers the session in the bulletin with its block count.
fn session_start(v: &Value, cfg: &Config) -> Result<Outcome> {
    let gcfg = guard_config(cfg);
    let session = v["session_id"].as_str().unwrap_or_default();
    if matches!(v["source"].as_str(), Some("compact" | "clear")) {
        acus_guard::hints::reset(&gcfg, session);
    }
    acus_guard::blocks::register(&gcfg, session);
    outprint!("{}", crate::cmd_skill::CORE);
    Ok(Outcome::Found)
}

/// Prints the core skill as context for a SubagentStart, which does not see SessionStart output.
fn subagent_start() -> Result<Outcome> {
    print_reply(&HookOutput {
        hook_event_name: "SubagentStart",
        additional_context: Some(Cow::Borrowed(crate::cmd_skill::CORE)),
        ..HookOutput::default()
    })?;
    Ok(Outcome::Found)
}

/// What the guard decided for one call, before it is logged and printed.
enum Decision {
    Rewrite(Rewrite),
    Verdict(Verdict),
    Pass,
}

/// The `hookSpecificOutput` object of a reply. Fields serialize in declaration order, which is the
/// order the hooks have always printed; a field that is not set is left out.
#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct HookOutput {
    hook_event_name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_context: Option<Cow<'static, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_decision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_decision_reason: Option<String>,
}

/// The line a hook prints: the reply wrapped in `hookSpecificOutput`.
#[derive(serde::Serialize)]
struct Reply<'a> {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: &'a HookOutput,
}

/// Answers a PreToolUse as the guard always has; a missing event (Codex) takes the same path.
fn answer(mut v: Value, cfg: &Config, config_problem: Option<&str>) -> Result<Outcome> {
    let cwd = v["cwd"].as_str();
    let scope = Scope::new(cwd);
    let gcfg = guard_config(cfg);
    let session = v["session_id"].as_str().unwrap_or_default();
    let note = config_problem.and_then(|p| config_note(&gcfg, session, p, &cfg.path));
    let decision = decide(&v, scope.as_ref(), cwd);
    let blocked = matches!(decision, Decision::Verdict(Verdict::Deny(..)));
    let reply = match decision {
        Decision::Rewrite(rw) => {
            log_decision(&gcfg, &v, &Verdict::Rewrite(rw.rule));
            let input = v.get_mut("tool_input").map(Value::take).unwrap_or_default();
            Some(rewrite_reply(input, rw, note.as_deref()))
        }
        // A pure hint repeats for every call of its rule, so `hints` shows only some of them. A call
        // whose hint is hidden passes silently and is not logged.
        Decision::Verdict(verdict) if shown(&gcfg, session, &verdict) => {
            log_decision(&gcfg, &v, &verdict);
            verdict_reply(verdict, note)
        }
        Decision::Verdict(_) | Decision::Pass => note_reply(note),
    };
    if let Some(reply) = reply {
        print_reply(&reply)?;
    }
    // Counted after the reply is printed, so the agent's decision never waits for the bulletin.
    if blocked {
        acus_guard::blocks::record(&gcfg, v["session_id"].as_str().unwrap_or_default());
    }
    Ok(Outcome::Found)
}

/// Decides one call: a Bash call goes through the rewrite and the rules, and every other tool
/// through its own check. `Pass` runs the call unchanged.
fn decide(v: &Value, scope: Option<&Scope>, cwd: Option<&str>) -> Decision {
    let input = &v["tool_input"];
    let command = input["command"].as_str().unwrap_or_default();
    // Codex hooks (they carry a `turn_id`) cannot rewrite a call, only refuse it.
    let can_rewrite = v["tool_name"].as_str() == Some("Bash") && v.get("turn_id").is_none();
    let checked = |verdict: Option<Verdict>| verdict.map_or(Decision::Pass, Decision::Verdict);
    match v["tool_name"].as_str() {
        Some("Read") => checked(acus_guard::read_check(input, cwd, scope)),
        Some("Grep") => checked(acus_guard::grep_tool(input, cwd)),
        Some("Glob") => Decision::Verdict(acus_guard::glob_tool()),
        Some("PowerShell") => checked(acus_guard::check_ps(command, scope)),
        _ => match acus_guard::decide_bash(command, scope, can_rewrite) {
            BashDecision::Rewrite(rw) => Decision::Rewrite(rw),
            BashDecision::Verdict(verdict) => Decision::Verdict(verdict),
            BashDecision::Pass => Decision::Pass,
        },
    }
}

fn log_decision(gcfg: &GuardConfig, hook: &Value, verdict: &Verdict) {
    // A log that cannot be written must not change the decision; `self doctor` reports it.
    let _ = acus_guard::log::append(gcfg, hook, verdict);
}

/// The reply to a rewritten call: the updated input, and the note that says what ran instead.
fn rewrite_reply(mut input: Value, rw: Rewrite, note: Option<&str>) -> HookOutput {
    input["command"] = rw.command.into();
    HookOutput {
        hook_event_name: "PreToolUse",
        updated_input: Some(input),
        // No permissionDecisionReason: an allow reason would reach only the user, not the agent.
        additional_context: Some(with_note(rw.note, note).into()),
        // A build is left undecided, so Claude Code's permission flow decides the rewritten call.
        permission_decision: (!rw.deferred).then_some("allow"),
        ..HookOutput::default()
    }
}

/// The reply to a verdict: a deny reason, or a hint for the agent's context. An escape or a
/// rewrite has no reply of its own, so only the config note is printed.
fn verdict_reply(verdict: Verdict, note: Option<String>) -> Option<HookOutput> {
    Some(match verdict {
        Verdict::Deny(_, reason) => HookOutput {
            hook_event_name: "PreToolUse",
            permission_decision: Some("deny"),
            permission_decision_reason: Some(with_note(reason, note.as_deref())),
            ..HookOutput::default()
        },
        // The command runs as usual; only the agent's context gains the hint.
        Verdict::Hint(_, hint) => HookOutput {
            hook_event_name: "PreToolUse",
            additional_context: Some(with_note(hint, note.as_deref()).into()),
            ..HookOutput::default()
        },
        Verdict::Escape(_) | Verdict::Rewrite(_) => return note_reply(note),
    })
}

/// The reply that carries only the config note, when there is one.
fn note_reply(note: Option<String>) -> Option<HookOutput> {
    note.map(|note| HookOutput {
        hook_event_name: "PreToolUse",
        additional_context: Some(note.into()),
        ..HookOutput::default()
    })
}

/// Prints one reply as a JSON line.
fn print_reply(hook: &HookOutput) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&Reply {
            hook_specific_output: hook
        })?
    );
    Ok(())
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
        hint_min: usize::MAX,
        hint_max: usize::MAX,
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
