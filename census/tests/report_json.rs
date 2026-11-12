// SPDX-License-Identifier: MPL-2.0
//! Pins the `--json` output of the report commands. Each expected document is the file of its
//! test name under `tests/report_json/`; `CENSUS_GOLDEN_UPDATE=1` writes the files from the binary.
//! Local times (the month, the weekday and hour buckets) use the offset `CENSUS_TZ_OFFSET_SECS`
//! gives, 7200 (Berlin summer time), so the goldens do not depend on the zone of the machine.

#![expect(
    clippy::expect_used,
    reason = "a golden test that cannot run or write its file must fail"
)]

mod common;

use common::Scratch;

/// Where census reads its config under a scratch home: macOS ignores `XDG_CONFIG_HOME`.
const CONFIG_FILE: &str = if cfg!(target_os = "macos") {
    "Library/Application Support/census/config.toml"
} else {
    "config/census/config.toml"
};
use serde_json::json;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// 2026-10-14 08:00 UTC, the clock of every run.
const NOW: &str = "1791964800";

const STATUS: &str = r#"{"session_id":"s1","model":{"id":"claude-opus-5-5","display_name":"Opus"},"context_window":{"total_input_tokens":44000,"context_window_size":1000000},"rate_limits":{"five_hour":{"used_percentage":6,"resets_at":1793530800},"seven_day":{"used_percentage":1,"resets_at":1794106800}}}"#;

/// Runs census in `cwd` with `home` as the user profile, state, config and Claude Code directory,
/// the clock at `NOW` and `stdin` as input. The goldens are pinned to a fixed +02:00 offset, so DST
/// is deliberately not modelled.
fn census(home: &Path, cwd: &Path, args: &[&str], stdin: &str) -> Output {
    census_at(home, cwd, args, stdin, "7200")
}

/// Runs census like `census`, with `CENSUS_TZ_OFFSET_SECS` set to `offset`.
fn census_at(home: &Path, cwd: &Path, args: &[&str], stdin: &str, offset: &str) -> Output {
    let at = |rel: &str| home.join(rel).display().to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_census"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", at("config"))
        .env("APPDATA", at("config"))
        .env("XDG_STATE_HOME", at("state"))
        .env("LOCALAPPDATA", at("state"))
        .env("CLAUDE_CONFIG_DIR", at("claude"))
        .env("CODEX_HOME", at("codex"))
        .env("CENSUS_NO_PRICE_REFRESH", "1")
        .env("CENSUS_NOW", NOW)
        .env("CENSUS_TZ_OFFSET_SECS", offset)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
        .env_remove("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
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

/// Checks the exit code and the stdout of a run against the golden file `name`. The scratch home
/// is shown as `<HOME>` so that the file does not depend on where the test ran.
fn golden(name: &str, out: &Output, home: &Path, code: i32) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let root = home.display().to_string();
    let resolved = home
        .canonicalize()
        .map_or_else(|_| root.clone(), |p| p.display().to_string());
    let resolved = resolved.strip_prefix(r"\\?\").unwrap_or(&resolved);
    let shown = String::from_utf8_lossy(&out.stdout)
        .replace(&resolved.replace('\\', r"\\"), "<HOME>")
        .replace(resolved, "<HOME>")
        .replace("Library/Application Support/census", "config/census")
        .replace(&root.replace('\\', r"\\"), "<HOME>")
        .replace(&root, "<HOME>")
        .replace(r"\\", "/");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/report_json");
    let file = dir.join(format!("{name}.json"));
    if std::env::var_os("CENSUS_GOLDEN_UPDATE").is_some() {
        std::fs::create_dir_all(&dir).expect("create the golden directory");
        std::fs::write(&file, &shown).expect("write the golden file");
    } else {
        let expected = std::fs::read_to_string(&file).expect("read the golden file");
        assert_eq!(shown, expected, "{name}");
    }
}

fn jsonl(records: &[serde_json::Value]) -> String {
    records.iter().map(|r| format!("{r}\n")).collect()
}

/// Two Claude Code sessions worth of requests in October 2026: a main transcript with tool calls,
/// a compaction and a worker spawn, and the own transcript of that worker.
fn claude_fixture(home: &Scratch) {
    home.write(
        "claude/projects/-work-app/s1.jsonl",
        &jsonl(&[
            json!({"type": "user", "cwd": "/work/app", "timestamp": "2026-10-02T09:00:00.000Z",
                "message": {"role": "user", "content": "go"}}),
            json!({"type": "assistant", "cwd": "/work/app", "timestamp": "2026-10-02T09:00:10.000Z",
                "message": {"id": "m1", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 1000, "cache_read_input_tokens": 59000,
                        "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 2000},
                        "output_tokens": 10},
                    "content": [{"type": "tool_use", "id": "t1", "name": "Grep", "input": {"pattern": "x"}},
                        {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "sed -n 1,5p a"}}]}}),
            json!({"type": "user", "timestamp": "2026-10-02T09:00:20.000Z",
                "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "0123456789"}]}}),
            json!({"type": "assistant", "timestamp": "2026-10-03T10:00:00.000Z",
                "message": {"id": "m2", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 500, "cache_read_input_tokens": 60000, "output_tokens": 20},
                    "content": [{"type": "tool_use", "id": "a1", "name": "Agent", "input": {"subagent_type": "worker"}},
                        {"type": "tool_use", "id": "t3", "name": "Read", "input": {"file_path": "a"}}]}}),
            json!({"type": "user", "timestamp": "2026-10-03T10:05:00.000Z",
                "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a1", "content": "done"}]}}),
            json!({"type": "assistant", "timestamp": "2026-10-03T10:06:00.000Z",
                "message": {"id": "m3", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 300, "cache_read_input_tokens": 61000,
                        "cache_creation": {"ephemeral_5m_input_tokens": 2000, "ephemeral_1h_input_tokens": 0},
                        "output_tokens": 30},
                    "content": [{"type": "tool_use", "id": "t4", "name": "Edit", "input": {}}]}}),
            json!({"type": "system", "subtype": "compact_boundary", "timestamp": "2026-10-03T10:07:00.000Z"}),
        ]),
    );
    home.write(
        "claude/projects/-work-app/s1/subagents/agent-a.jsonl",
        &jsonl(&[json!({"type": "assistant", "attributionAgent": "worker",
            "timestamp": "2026-10-03T10:05:10.000Z",
            "message": {"id": "w1", "model": "claude-sonnet-5-5",
                "usage": {"input_tokens": 100, "cache_read_input_tokens": 5000, "output_tokens": 50},
                "content": [{"type": "tool_use", "id": "t5", "name": "Bash", "input": {"command": "acus find x"}}]}}),
        ]),
    );
}

/// The acus guard log: a deny, a hint and an escaped call, in the format acus writes.
fn guard_fixture(home: &Scratch) {
    home.write(
        "state/acus/guard.jsonl",
        &jsonl(&[
            json!({"ts": 1791900000, "session": "s1", "cwd": "/work/app", "tool": "Bash",
                "rule": "grep", "kind": "deny", "call": "grep -r x ."}),
            json!({"ts": 1791900100, "session": "s1", "cwd": "/work/app", "tool": "Bash",
                "rule": "git-log", "kind": "hint", "call": "git log -5"}),
            json!({"ts": 1791900200, "session": "s1", "cwd": "/work/app", "tool": "Bash",
                "rule": "sed", "kind": "escape", "call": "sed -n 1,5p a\nb"}),
        ]),
    );
}

/// Two requests of 400k input tokens in October 2026, and a census config with `config`.
fn budget_fixture(home: &Scratch, config: &str) {
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
            request("2026-10-14T06:00:00.000Z", "m3"),
        ]),
    );
    home.write(CONFIG_FILE, config);
}

/// A session recorded in `cwd`: a main transcript with 60k context on `claude-opus-5-5`, one
/// worker subagent, and a 400k `autoCompactWindow`.
fn session_fixture(home: &Scratch, cwd: &Path) {
    let recorded = cwd
        .canonicalize()
        .expect("the working directory exists")
        .display()
        .to_string();
    // On Windows `canonicalize` adds a verbatim prefix that Claude Code never records.
    let recorded = recorded
        .strip_prefix(r"\\?\")
        .unwrap_or(&recorded)
        .to_owned();
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
}

#[test]
fn usage_json_is_unchanged() {
    let home = Scratch::new("golden-usage");
    claude_fixture(&home);
    let out = census(home.path(), home.path(), &["--json", "usage"], "");
    golden("usage", &out, home.path(), 0);
}

#[test]
fn usage_views_json_are_unchanged() {
    let home = Scratch::new("golden-usage-views");
    claude_fixture(&home);
    let run = |args: &[&str]| census(home.path(), home.path(), args, "");
    golden(
        "usage_tools",
        &run(&["--json", "usage", "--tools"]),
        home.path(),
        0,
    );
    golden(
        "usage_agents",
        &run(&["--json", "usage", "--agents"]),
        home.path(),
        0,
    );
    golden(
        "usage_handbacks",
        &run(&["--json", "usage", "--handbacks"]),
        home.path(),
        0,
    );
    golden(
        "usage_compare",
        &run(&["--json", "usage", "--compare", "2026-10-03"]),
        home.path(),
        0,
    );
    for kind in ["weekday", "hour", "cycle"] {
        let out = run(&["--json", "usage", "--by", kind]);
        golden(&format!("usage_by_{kind}"), &out, home.path(), 0);
    }
}

/// The hour buckets follow the offset: the fixtures' events are at 09:00 and 10:00 UTC, so under
/// offset 0 they fill the 09:00 and 10:00 buckets, and under 7200 the 11:00 and 12:00 ones.
#[test]
fn usage_hour_buckets_follow_the_offset() {
    let home = Scratch::new("golden-offset");
    claude_fixture(&home);
    let filled = |offset: &str| -> Vec<String> {
        let out = census_at(
            home.path(),
            home.path(),
            &["--json", "usage", "--by", "hour"],
            "",
            offset,
        );
        assert_eq!(out.status.code(), Some(0));
        let doc: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("census prints JSON");
        doc["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .filter(|row| row["usd"].as_f64().unwrap_or(0.0) > 0.0)
            .map(|row| row["label"].as_str().expect("label").to_owned())
            .collect()
    };
    assert_eq!(filled("0"), ["09:00", "10:00"]);
    assert_eq!(filled("7200"), ["11:00", "12:00"]);
}

#[test]
fn usage_guard_json_is_unchanged() {
    let home = Scratch::new("golden-guard");
    guard_fixture(&home);
    let out = census(
        home.path(),
        home.path(),
        &["--json", "usage", "--guard"],
        "",
    );
    golden("usage_guard", &out, home.path(), 0);
}

#[test]
fn limits_json_is_unchanged_in_plan_and_budget_mode() {
    let home = Scratch::new("golden-limits");
    claude_fixture(&home);
    let line = census(home.path(), home.path(), &["statusline", "--line"], STATUS);
    assert_eq!(line.status.code(), Some(0));
    let out = census(home.path(), home.path(), &["--json", "limits"], "");
    golden("limits", &out, home.path(), 0);

    let empty = Scratch::new("golden-limits-empty");
    let out = census(empty.path(), empty.path(), &["--json", "limits"], "");
    golden("limits_empty", &out, empty.path(), 1);

    let budget = Scratch::new("golden-limits-budget");
    budget_fixture(&budget, "mode = \"budget\"\nmonthly_budget_usd = 100.0\n");
    let out = census(budget.path(), budget.path(), &["--json", "limits"], "");
    golden("limits_budget", &out, budget.path(), 0);

    let fixed = Scratch::new("golden-limits-fixed");
    budget_fixture(
        &fixed,
        "mode = \"budget\"\nmonthly_budget_usd = 100.0\ndaily_limit_usd = 8.0\n",
    );
    let out = census(fixed.path(), fixed.path(), &["--json", "limits"], "");
    golden("limits_budget_fixed_daily", &out, fixed.path(), 0);
}

#[test]
fn context_and_session_json_are_unchanged() {
    let home = Scratch::new("golden-session");
    let cwd = home.path().join("work");
    std::fs::create_dir_all(&cwd).expect("create the working directory");
    session_fixture(&home, &cwd);
    let out = census(home.path(), &cwd, &["--json", "context"], "");
    golden("context", &out, home.path(), 0);
    let out = census(home.path(), &cwd, &["--json", "session"], "");
    golden("session", &out, home.path(), 0);
}

#[test]
fn prune_dry_run_json_is_unchanged() {
    let home = Scratch::new("golden-prune");
    budget_fixture(&home, "mode = \"plan\"\n");
    let out = census(
        home.path(),
        home.path(),
        &["--json", "prune", "--before", "2026-10-10", "--dry-run"],
        "",
    );
    golden("prune_dry_run", &out, home.path(), 0);
}

#[test]
fn self_config_show_json_is_unchanged() {
    let plan = Scratch::new("golden-config-plan");
    let out = census(
        plan.path(),
        plan.path(),
        &["--json", "self", "config", "show"],
        "",
    );
    golden("self_config", &out, plan.path(), 0);

    let budget = Scratch::new("golden-config-budget");
    budget_fixture(
        &budget,
        "mode = \"budget\"\nmonthly_budget_usd = 100.0\ndaily_limit_usd = 8.0\n",
    );
    let out = census(
        budget.path(),
        budget.path(),
        &["--json", "self", "config", "show"],
        "",
    );
    golden("self_config_budget", &out, budget.path(), 0);
}
