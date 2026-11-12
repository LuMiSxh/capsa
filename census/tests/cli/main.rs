// SPDX-License-Identifier: MPL-2.0
//! Runs the census binary with its state, its Claude Code configuration and its working directory
//! under a scratch home.

#[path = "../common/mod.rs"]
mod common;

mod billing;
mod context;
mod guard;
mod limits;
mod prune;
mod self_cmd;
mod statusline;
mod usage;

use common::Scratch;

/// Where census reads its config under a scratch home: macOS ignores `XDG_CONFIG_HOME`.
const CONFIG_FILE: &str = if cfg!(target_os = "macos") {
    "Library/Application Support/census/config.toml"
} else {
    "config/census/config.toml"
};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const STATUS: &str = r#"{"session_id":"s1","model":{"id":"claude-opus-5-5","display_name":"Opus"},"context_window":{"total_input_tokens":44000,"context_window_size":1000000},"rate_limits":{"five_hour":{"used_percentage":6,"resets_at":1793530800},"seven_day":{"used_percentage":1,"resets_at":1794106800}}}"#;

/// Runs census in `cwd` with `home` as the user profile, the state root at `home/state` and
/// `CLAUDE_CONFIG_DIR` at `home/claude`, feeding `stdin`.
#[expect(
    clippy::expect_used,
    reason = "a census binary that cannot run must fail the test"
)]
fn census(home: &Path, cwd: &Path, args: &[&str], stdin: &str, extra: &[(&str, &str)]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_census"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("LOCALAPPDATA", home.join("state"))
        .env("USERPROFILE", home)
        .env("APPDATA", home.join("config"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
        .env_remove("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
        .env("CENSUS_NO_PRICE_REFRESH", "1")
        .envs(extra.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start census");
    if let Some(mut input) = child.stdin.take() {
        // A command that exits before reading gives a broken pipe, which the test does not need.
        let _ = input.write_all(stdin.as_bytes());
    }
    child.wait_with_output().expect("wait for census")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn jsonl(records: &[Value]) -> String {
    records.iter().map(|r| format!("{r}\n")).collect()
}

/// Writes a session recorded in `cwd` into `home`: a main transcript with 60k context on
/// `claude-opus-5-5`, one worker subagent, and a 400k `autoCompactWindow`. Returns the session id.
#[expect(
    clippy::expect_used,
    reason = "a fixture that cannot be written must fail the test"
)]
fn claude_session(home: &Scratch, cwd: &Path) -> &'static str {
    let recorded = cwd.canonicalize().expect("the working directory exists");
    // On Windows `canonicalize` adds a verbatim prefix that Claude Code never records.
    let recorded = recorded.display().to_string();
    let recorded = recorded
        .strip_prefix("\u{5c}\u{5c}?\u{5c}")
        .unwrap_or(&recorded)
        .to_string();
    home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            json!({"type": "user", "cwd": recorded, "timestamp": "2026-10-14T08:00:00.000Z",
                "message": {"role": "user", "content": "go"}}),
            json!({"type": "assistant", "timestamp": "2026-10-14T08:00:10.000Z",
                "message": {"id": "m1", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 1000, "cache_read_input_tokens": 59000, "output_tokens": 10}}}),
        ]),
    );
    home.write(
        "claude/projects/app/s1/subagents/agent-a.jsonl",
        &jsonl(&[json!({"type": "assistant", "attributionAgent": "worker",
            "timestamp": "2026-10-14T08:01:00.000Z",
            "message": {"id": "m2", "model": "claude-sonnet-5-5",
                "usage": {"input_tokens": 1, "output_tokens": 1}}})]),
    );
    "s1"
}

/// The environment that points census's config, state, home and clock at `home`, with the clock
/// at 2026-10-14 08:00 UTC.
fn isolated_env(home: &Path) -> Vec<(&'static str, String)> {
    let at = |rel: &str| home.join(rel).display().to_string();
    vec![
        ("HOME", home.display().to_string()),
        ("USERPROFILE", home.display().to_string()),
        ("XDG_CONFIG_HOME", at("config")),
        ("APPDATA", at("config")),
        ("LOCALAPPDATA", at("state")),
        ("CENSUS_NOW", "1791964800".to_owned()),
        ("CENSUS_NO_PRICE_REFRESH", "1".to_owned()),
    ]
}

/// Writes a config in `mode` with a $100 budget, and two October 2026 requests of 400k input
/// tokens. They are older than the raw retention, so they fold into hourly aggregates.
fn october_spend(home: &Scratch, mode: &str) {
    let request = |timestamp: &str, id: &str| {
        json!({"type": "assistant", "timestamp": timestamp,
            "message": {"id": id, "model": "claude-opus-5-5",
                "usage": {"input_tokens": 400_000, "output_tokens": 10}}})
    };
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            request("2026-10-02T10:00:00.000Z", "m1"),
            request("2026-10-03T10:00:00.000Z", "m2"),
        ]),
    );
    home.write(
        CONFIG_FILE,
        &format!("mode = \"{mode}\"\nmonthly_budget_usd = 100.0\n"),
    );
}

#[expect(
    clippy::expect_used,
    reason = "output that is not the prune JSON must fail the test"
)]
fn removed(out: &Output) -> u64 {
    serde_json::from_str::<Value>(&text(&out.stdout)).expect("json")["hourly_removed"]
        .as_u64()
        .expect("a count of hourly aggregates")
}
