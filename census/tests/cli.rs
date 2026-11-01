// SPDX-License-Identifier: MPL-2.0
//! Runs the census binary with its state, its Claude Code configuration and its working directory
//! under a scratch home.

mod common;

use common::Scratch;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const STATUS: &str = r#"{"session_id":"s1","model":{"id":"claude-opus-5-5","display_name":"Opus"},"context_window":{"total_input_tokens":44000,"context_window_size":1000000},"rate_limits":{"five_hour":{"used_percentage":6,"resets_at":1793530800},"seven_day":{"used_percentage":1,"resets_at":1794106800}}}"#;

/// Runs census in `cwd` with `XDG_STATE_HOME` at `home/state` and `CLAUDE_CONFIG_DIR` at
/// `home/claude`, feeding `stdin`.
#[expect(
    clippy::expect_used,
    reason = "a census binary that cannot run must fail the test"
)]
fn census(home: &Path, cwd: &Path, args: &[&str], stdin: &str, extra: &[(&str, &str)]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_census"))
        .args(args)
        .current_dir(cwd)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
        .env_remove("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
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
    let recorded = recorded.display().to_string();
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

#[test]
fn statusline_records_snapshots_once_per_change() {
    let home = Scratch::new("cli-statusline");
    let cwd = home.path().to_path_buf();
    home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
    let out = census(home.path(), &cwd, &["statusline", "--line"], STATUS, &[]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout), "ctx 44k/400k · 5h 6% · wk 1%\n");
    let file = home.path().join("state/census/snapshots");
    assert_eq!(
        std::fs::read_to_string(&file)
            .expect("snapshots")
            .lines()
            .count(),
        2
    );

    census(home.path(), &cwd, &["statusline"], STATUS, &[]);
    assert_eq!(
        std::fs::read_to_string(&file)
            .expect("snapshots")
            .lines()
            .count(),
        2
    );
}

#[test]
fn statusline_is_silent_by_default_and_exits_zero_on_bad_input() {
    let home = Scratch::new("cli-silent");
    let cwd = home.path().to_path_buf();
    let quiet = census(home.path(), &cwd, &["statusline"], STATUS, &[]);
    assert_eq!(quiet.status.code(), Some(0));
    assert!(quiet.stdout.is_empty());
    let bad = census(
        home.path(),
        &cwd,
        &["statusline", "--line"],
        "not json",
        &[],
    );
    assert_eq!(bad.status.code(), Some(0));
    assert!(bad.stdout.is_empty());
}

#[test]
fn statusline_still_prints_the_line_when_the_state_dir_is_unwritable() {
    let home = Scratch::new("cli-unwritable");
    let cwd = home.path().to_path_buf();
    home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
    home.write("state", "a file, not a directory");
    let out = census(home.path(), &cwd, &["statusline", "--line"], STATUS, &[]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout), "ctx 44k/400k · 5h 6% · wk 1%\n");
}

#[test]
fn prune_needs_exactly_one_cutoff() {
    let home = Scratch::new("cli-prune-args");
    let cwd = home.path().to_path_buf();
    let none = census(home.path(), &cwd, &["prune"], "", &[]);
    assert_eq!(none.status.code(), Some(2));
    let both = census(
        home.path(),
        &cwd,
        &["prune", "--before", "2026-01-01", "--keep", "5"],
        "",
        &[],
    );
    assert_eq!(both.status.code(), Some(2));
}

#[test]
fn prune_rejects_a_bad_date_and_zero_days() {
    let home = Scratch::new("cli-prune-values");
    let cwd = home.path().to_path_buf();
    let date = census(
        home.path(),
        &cwd,
        &["prune", "--before", "2026-13-45"],
        "",
        &[],
    );
    assert_eq!(date.status.code(), Some(2));
    assert!(text(&date.stderr).contains("YYYY-MM-DD"));
    let days = census(home.path(), &cwd, &["prune", "--keep", "0"], "", &[]);
    assert_eq!(days.status.code(), Some(2));
    assert!(text(&days.stderr).contains("at least 1 day"));
}

#[test]
fn prune_dry_run_on_an_empty_ledger_removes_nothing() {
    let home = Scratch::new("cli-prune-empty");
    let cwd = home.path().to_path_buf();
    let out = census(
        home.path(),
        &cwd,
        &["prune", "--keep", "365", "--dry-run"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stdout),
        "would remove 0 hourly aggregates and 0 snapshots older than 365 days\n"
    );
}

#[test]
fn context_reads_the_session_of_this_directory() {
    let home = Scratch::new("cli-context");
    let cwd = home.path().join("work");
    std::fs::create_dir_all(&cwd).expect("create the working directory");
    let id = claude_session(&home, &cwd);

    let out = census(home.path(), &cwd, &["context"], "", &[]);
    assert_eq!(out.status.code(), Some(0));
    let shown = text(&out.stdout);
    assert!(shown.contains("60k of 400k (15%), 340k left"), "{shown}");
    assert!(
        shown.contains("400k from autoCompactWindow in settings.json"),
        "{shown}"
    );
    assert!(shown.contains("claude-opus-5-5"), "{shown}");
    assert!(
        shown.contains(&format!("{id} (most recent transcript")),
        "{shown}"
    );

    let json = census(home.path(), &cwd, &["--json", "context"], "", &[]);
    let shown = text(&json.stdout);
    assert!(
        shown.contains("\"tokens\":60000") && shown.contains("\"picked\":\"recent\""),
        "{shown}"
    );

    let by_env = census(
        home.path(),
        &cwd,
        &["context"],
        "",
        &[("CLAUDE_SESSION_ID", id)],
    );
    assert!(text(&by_env.stdout).contains("(CLAUDE_SESSION_ID)"));
}

#[test]
fn session_summarizes_the_session_of_this_directory() {
    let home = Scratch::new("cli-session");
    let cwd = home.path().join("work");
    std::fs::create_dir_all(&cwd).expect("create the working directory");
    claude_session(&home, &cwd);
    let out = census(home.path(), &cwd, &["session"], "", &[]);
    assert_eq!(out.status.code(), Some(0));
    let shown = text(&out.stdout);
    assert!(shown.contains("2 (1 subagents)"), "{shown}");
    assert!(shown.contains("worker 1"), "{shown}");
    assert!(shown.contains("compactions 0"), "{shown}");
    assert!(shown.contains("60k of 400k"), "{shown}");
}

#[test]
fn context_and_session_name_an_unknown_session() {
    let home = Scratch::new("cli-unknown");
    let cwd = home.path().to_path_buf();
    let out = census(
        home.path(),
        &cwd,
        &["session", "--session", "nope"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("no transcript for session nope"));
}
