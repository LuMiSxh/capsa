// SPDX-License-Identifier: MPL-2.0
//! Runs the census binary with its state, its Claude Code configuration and its working directory
//! under a scratch home.

mod common;

use common::Scratch;

/// Where census reads its config under a scratch home: macOS ignores `XDG_CONFIG_HOME`.
const CONFIG_FILE: &str = if cfg!(target_os = "macos") {
    "Library/Application Support/census/config.json"
} else {
    "config/census/config.json"
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

#[test]
fn guard_log_counts_rotated_and_current_lines_and_skips_bad_ones() {
    let home = Scratch::new("cli-guard");
    let cwd = home.path().to_path_buf();
    home.write(
        "acus/guard.jsonl.old",
        &jsonl(&[json!({"ts": 10, "rule": "grep", "kind": "deny", "call": "grep a"})]),
    );
    home.write(
        "acus/guard.jsonl",
        &format!(
            "{}not json\n{}",
            jsonl(&[json!({"ts": 20, "rule": "grep", "kind": "hint", "call": "grep b"})]),
            jsonl(&[json!({"ts": 30, "rule": "sed", "kind": "escape", "call": "sed -i x"})]),
        ),
    );
    let config = home.path().join("acus/config.toml").display().to_string();
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--json"],
        "",
        &[("ACUS_CONFIG", config.as_str())],
    );
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    let rules = v["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["deny"], 1);
    assert_eq!(rules[0]["hint"], 1);
    assert_eq!(rules[1]["escape"], 1);
    assert_eq!(v["escaped"].as_array().map(Vec::len), Some(1));
}

#[test]
fn guard_log_reads_the_current_line_format() {
    let home = Scratch::new("cli-guard-current");
    let cwd = home.path().to_path_buf();
    home.write(
        "acus/guard.jsonl",
        &jsonl(&[
            json!({"ts": 1, "level": "info", "event": "decision", "session": "s", "cwd": "c",
            "tool": "Bash", "rule": "grep", "kind": "rewrite", "call": "x"}),
        ]),
    );
    let config = home.path().join("acus/config.toml").display().to_string();
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--json"],
        "",
        &[("ACUS_CONFIG", config.as_str())],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    let rules = v["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["rewrite"], 1);
}

#[test]
fn the_config_command_moved_under_self() {
    let home = Scratch::new("cli-config-moved");
    let cwd = home.path().to_path_buf();
    let old = census(home.path(), &cwd, &["config", "show"], "", &[]);
    assert_eq!(old.status.code(), Some(2), "{}", text(&old.stderr));
    let new = census(home.path(), &cwd, &["self", "config", "show"], "", &[]);
    assert_eq!(new.status.code(), Some(0), "{}", text(&new.stderr));
}

#[test]
fn doctor_is_one_json_value_and_exits_1_only_when_a_check_fails() {
    let home = Scratch::new("cli-doctor");
    let cwd = home.path().to_path_buf();
    let home_str = home.path().to_str().expect("utf-8 path");
    let extra = [("HOME", home_str)];
    let setup = census(home.path(), &cwd, &["self", "setup"], "", &extra);
    assert_eq!(setup.status.code(), Some(0), "{}", text(&setup.stderr));

    let healthy = census(home.path(), &cwd, &["self", "doctor", "--json"], "", &extra);
    assert_eq!(healthy.status.code(), Some(0), "{}", text(&healthy.stderr));
    let v: Value = serde_json::from_slice(&healthy.stdout).expect("doctor --json is one value");
    assert_eq!(v["tool"], "census");
    assert_eq!(v["ok"], true);
    let groups: Vec<&str> = v["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .map(|g| g["name"].as_str().expect("group name"))
        .collect();
    assert_eq!(groups, ["install", "config", "skill", "hooks", "state"]);

    home.write(CONFIG_FILE, "{not json");
    let broken = census(home.path(), &cwd, &["self", "doctor", "--json"], "", &extra);
    assert_eq!(broken.status.code(), Some(1), "{}", text(&broken.stderr));
    let v: Value = serde_json::from_slice(&broken.stdout).expect("doctor --json is one value");
    assert_eq!(v["ok"], false);
    assert_eq!(v["groups"][1]["name"], "config");
    assert_eq!(v["groups"][1]["checks"][0]["status"], "fail");
}

#[test]
fn uninstall_dry_run_prints_one_json_value_and_removes_nothing() {
    let home = Scratch::new("cli-uninstall-dry");
    let cwd = home.path().to_path_buf();
    let home_str = home.path().to_str().expect("utf-8 path");
    let extra = [("HOME", home_str)];
    let skill = census(
        home.path(),
        &cwd,
        &["self", "skill", "--install"],
        "",
        &extra,
    );
    assert_eq!(skill.status.code(), Some(0), "{}", text(&skill.stderr));

    let out = census(
        home.path(),
        &cwd,
        &["self", "uninstall", "--dry-run", "--json"],
        "",
        &extra,
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("uninstall --json is one value");
    assert_eq!(v["removed"], false);
    assert!(home.path().join(".claude/skills/census/SKILL.md").is_file());
}

#[test]
fn guard_log_reads_on_past_a_line_that_is_not_utf8() {
    let home = Scratch::new("cli-guard-utf8");
    let cwd = home.path().to_path_buf();
    let mut bytes =
        jsonl(&[json!({"ts": 20, "rule": "grep", "kind": "hint", "call": "grep b"})]).into_bytes();
    bytes.extend_from_slice(b"\xff\xfe not utf-8\n");
    bytes.extend_from_slice(
        jsonl(&[json!({"ts": 30, "rule": "sed", "kind": "escape", "call": "sed -i x"})]).as_bytes(),
    );
    home.write_bytes("acus/guard.jsonl", &bytes);
    let config = home.path().join("acus/config.toml").display().to_string();
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--json"],
        "",
        &[("ACUS_CONFIG", config.as_str())],
    );
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    let rules = v["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["hint"], 1);
    assert_eq!(rules[1]["escape"], 1);
}

#[test]
fn daily_limit_is_set_shown_and_cleared() {
    let home = Scratch::new("cli-daily-set");
    let cfg = home.path().join("config");
    let state = home.path().join("state");
    let extra = [
        ("XDG_CONFIG_HOME", cfg.to_str().expect("utf-8 path")),
        ("APPDATA", cfg.to_str().expect("utf-8 path")),
        ("LOCALAPPDATA", state.to_str().expect("utf-8 path")),
    ];
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &extra);
    let saved = || -> Value {
        let text = std::fs::read_to_string(home.path().join(CONFIG_FILE)).expect("config file");
        serde_json::from_str(&text).expect("config json")
    };

    assert_eq!(
        run(&["self", "config", "set", "mode", "budget"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["self", "config", "set", "monthly-budget", "100"])
            .status
            .code(),
        Some(0)
    );
    let set = run(&["self", "config", "set", "daily-limit", "8"]);
    assert_eq!(set.status.code(), Some(0));
    assert!(text(&set.stdout).contains("$8.00"), "{}", text(&set.stdout));
    assert_eq!(saved()["daily_limit_usd"], 8.0);
    assert_eq!(
        run(&["self", "config", "set", "daily-limit", "0"])
            .status
            .code(),
        Some(2)
    );
    let cleared = run(&["self", "config", "set", "daily-limit", "auto"]);
    assert_eq!(cleared.status.code(), Some(0));
    assert!(saved()["daily_limit_usd"].is_null());
    assert!(
        text(&cleared.stdout).contains("auto"),
        "{}",
        text(&cleared.stdout)
    );
}

#[test]
fn budget_limits_json_carries_today_and_its_daily_limit() {
    let home = Scratch::new("cli-daily-json");
    october_spend(&home, "budget");
    let env = isolated_env(home.path());
    let extra: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let limits = |extra: &[(&str, &str)]| {
        let out = census(home.path(), home.path(), &["--json", "limits"], "", extra);
        assert_eq!(out.status.code(), Some(0));
        serde_json::from_slice::<Value>(&out.stdout).expect("json")
    };

    let derived = limits(&extra);
    assert_eq!(derived["today"]["source"], "derived");
    assert_eq!(derived["today"]["usd"], 0.0);
    assert!(derived["today"]["limit"].as_f64().is_some_and(|l| l > 0.0));

    home.write(
        CONFIG_FILE,
        r#"{"mode":"budget","monthly_budget_usd":100.0,"daily_limit_usd":8.0}"#,
    );
    let fixed = limits(&extra);
    assert_eq!(fixed["today"]["source"], "fixed");
    assert_eq!(fixed["today"]["limit"], 8.0);
}

#[test]
fn statusline_records_snapshots_once_per_change() {
    let home = Scratch::new("cli-statusline");
    let cwd = home.path().to_path_buf();
    home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
    let out = census(home.path(), &cwd, &["statusline", "--line"], STATUS, &[]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout), "ctx 44k/400k · 5h 6% · wk 1%\n");
    let file = home.path().join(if cfg!(windows) {
        "state/census/state/snapshots"
    } else {
        "state/census/snapshots"
    });
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
    assert!(shown.contains("compactions: 0"), "{shown}");
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

#[test]
fn limits_and_usage_by_without_data_exit_one() {
    let home = Scratch::new("limits-empty");
    let out = census(home.path(), home.path(), &["limits"], "", &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stdout).contains("no usage data yet"));
    let out = census(home.path(), home.path(), &["--json", "limits"], "", &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stdout).contains("\"data\":false"));
    let out = census(
        home.path(),
        home.path(),
        &["usage", "--by", "cycle"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn budget_mode_switches_limits_and_statusline() {
    let home = Scratch::new("cli-budget");
    let cfg = home.path().join("config");
    let state = home.path().join("state");
    home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
    let extra = [
        ("XDG_CONFIG_HOME", cfg.to_str().expect("utf-8 path")),
        ("APPDATA", cfg.to_str().expect("utf-8 path")),
        ("LOCALAPPDATA", state.to_str().expect("utf-8 path")),
    ];
    let run = |args: &[&str], stdin: &str| census(home.path(), home.path(), args, stdin, &extra);

    let shown = text(&run(&["self", "config", "show"], "").stdout);
    assert!(shown.contains("plan"), "{shown}");
    assert_eq!(
        run(&["self", "config", "set", "mode", "budget"], "")
            .status
            .code(),
        Some(0)
    );
    let set = run(&["self", "config", "set", "monthly-budget", "200"], "");
    assert_eq!(set.status.code(), Some(0));
    assert!(text(&set.stdout).contains("$200.00"));
    assert_eq!(
        run(&["self", "config", "set", "monthly-budget", "-5"], "")
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(&["self", "config", "set", "mode", "free"], "")
            .status
            .code(),
        Some(2)
    );

    let json: Value =
        serde_json::from_str(&text(&run(&["--json", "limits"], "").stdout)).expect("json");
    assert_eq!(json["mode"], "budget");
    assert_eq!(json["budget_usd"], 200.0);

    let line = run(&["statusline", "--line"], STATUS);
    assert_eq!(text(&line.stdout), "ctx 44k/400k · $0/$200 0% →$0\n");

    run(&["self", "config", "set", "mode", "plan"], "");
    let line = run(&["statusline", "--line"], STATUS);
    assert_eq!(text(&line.stdout), "ctx 44k/400k · 5h 6% · wk 1%\n");
}

#[test]
fn set_billing_stores_the_provider_and_warns_about_an_unknown_one() {
    let home = Scratch::new("cli-billing-set");
    let cfg = home.path().join("config");
    let state = home.path().join("state");
    let extra = [
        ("XDG_CONFIG_HOME", cfg.to_str().expect("utf-8 path")),
        ("APPDATA", cfg.to_str().expect("utf-8 path")),
        ("LOCALAPPDATA", state.to_str().expect("utf-8 path")),
    ];
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &extra);

    let shown = run(&["self", "config", "show"]);
    assert!(
        text(&shown.stdout).contains("anthropic subscription"),
        "{}",
        text(&shown.stdout)
    );

    let set = run(&["self", "config", "set", "billing", "openai", "subscription"]);
    assert_eq!(set.status.code(), Some(0), "{}", text(&set.stderr));
    assert!(
        text(&set.stdout).contains("anthropic subscription, openai subscription"),
        "{}",
        text(&set.stdout)
    );
    assert!(
        !text(&set.stdout).contains("warning"),
        "{}",
        text(&set.stdout)
    );

    let unknown = run(&["self", "config", "set", "billing", "mistral", "api"]);
    assert_eq!(unknown.status.code(), Some(0), "{}", text(&unknown.stderr));
    assert!(
        text(&unknown.stdout).contains("`mistral` is not in the price table"),
        "{}",
        text(&unknown.stdout)
    );

    let saved: Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join(CONFIG_FILE)).expect("config file"),
    )
    .expect("config json");
    assert_eq!(
        saved["billing"],
        json!({"anthropic": "subscription", "openai": "subscription", "mistral": "api"})
    );

    let anthropic = run(&[
        "--json",
        "self",
        "config",
        "set",
        "billing",
        "anthropic",
        "api",
    ]);
    assert_eq!(
        anthropic.status.code(),
        Some(0),
        "{}",
        text(&anthropic.stderr)
    );
    let json: Value = serde_json::from_str(&text(&anthropic.stdout)).expect("json");
    assert_eq!(json["billing"]["anthropic"], "api");
    assert!(json.get("warnings").is_none(), "{json}");
}

#[test]
fn set_billing_refuses_a_bad_provider_or_billing_with_a_hint() {
    let home = Scratch::new("cli-billing-bad");
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &[]);
    for args in [
        ["self", "config", "set", "billing", "OpenAI", "api"],
        ["self", "config", "set", "billing", "openai", "free"],
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("hint:"), "{}", text(&out.stderr));
    }
    assert!(!home.path().join(CONFIG_FILE).exists());
}

#[test]
fn usage_names_a_provider_billed_as_subscription_by_the_setting() {
    let home = Scratch::new("cli-billing-usage");
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            json!({"type": "assistant", "timestamp": "2026-10-14T08:00:10.000Z",
                "message": {"id": "m1", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 1000, "output_tokens": 10}}}),
            json!({"type": "assistant", "timestamp": "2026-10-14T08:01:10.000Z",
                "message": {"id": "m2", "model": "gpt-5",
                    "usage": {"input_tokens": 1000, "output_tokens": 10}}}),
        ]),
    );
    let providers = |out: &Output| -> Vec<String> {
        text(&out.stdout)
            .lines()
            .find(|l| l.trim_start().starts_with("providers"))
            .map(|l| {
                l.trim_start()["providers".len()..]
                    .trim_start_matches(':')
                    .trim()
                    .split(", ")
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };

    let plain = census(home.path(), home.path(), &["usage"], "", &[]);
    assert_eq!(plain.status.code(), Some(0), "{}", text(&plain.stderr));
    let row = providers(&plain);
    assert!(
        row.iter()
            .any(|p| p.starts_with("openai $") && p.ends_with("(API)")),
        "{row:?}"
    );

    home.write(
        CONFIG_FILE,
        r#"{"billing":{"anthropic":"subscription","openai":"subscription"}}"#,
    );
    let set = census(home.path(), home.path(), &["usage"], "", &[]);
    assert_eq!(set.status.code(), Some(0), "{}", text(&set.stderr));
    let row = providers(&set);
    assert!(
        row.iter()
            .any(|p| p.starts_with("openai $") && p.ends_with("(subscription)")),
        "{row:?}"
    );
}

#[test]
fn limits_ignore_a_provider_billed_as_subscription() {
    let home = Scratch::new("cli-billing-limits");
    let request = |model: &str, id: &str, at: &str| {
        json!({"type": "assistant", "timestamp": at,
            "message": {"id": id, "model": model,
                "usage": {"input_tokens": 400_000, "output_tokens": 10}}})
    };
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            request("claude-opus-5-5", "m1", "2026-10-02T10:00:00.000Z"),
            request("gpt-5", "m2", "2026-10-03T10:00:00.000Z"),
        ]),
    );
    let owned = isolated_env(home.path());
    let env: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let spent = |billing: &str| -> f64 {
        home.write(
            CONFIG_FILE,
            &format!(r#"{{"mode":"budget","monthly_budget_usd":100.0,"billing":{billing}}}"#),
        );
        let out = census(home.path(), home.path(), &["--json", "limits"], "", &env);
        let json: Value = serde_json::from_str(&text(&out.stdout)).expect("json");
        json["spent_usd"].as_f64().expect("spent_usd")
    };
    let api = spent(r#"{"anthropic":"subscription"}"#);
    let subscription = spent(r#"{"anthropic":"subscription","openai":"subscription"}"#);
    assert!(api > 0.0, "{api}");
    assert_eq!(subscription, api);
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
        &format!(r#"{{"mode":"{mode}","monthly_budget_usd":100.0}}"#),
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

#[test]
fn budget_mode_prune_keeps_the_current_months_aggregates_and_spend() {
    let home = Scratch::new("cli-prune-month");
    october_spend(&home, "budget");
    let owned = isolated_env(home.path());
    let env: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &env);
    let spend = || -> f64 {
        serde_json::from_str::<Value>(&text(&run(&["--json", "limits"]).stdout))
            .expect("json")["spent_usd"]
            .as_f64()
            .expect("a month spend")
    };
    let before = spend();
    assert!(before > 0.0, "{before}");

    let by_date = run(&["--json", "prune", "--before", "2026-10-10"]);
    assert_eq!(by_date.status.code(), Some(1));
    assert_eq!(removed(&by_date), 0);
    assert!(text(&by_date.stderr).contains("budget mode keeps"));

    let by_days = run(&["--json", "prune", "--keep", "1"]);
    assert_eq!(by_days.status.code(), Some(1));
    assert_eq!(removed(&by_days), 0);

    assert_eq!(spend(), before);
}

#[test]
fn plan_mode_prune_removes_the_october_aggregates() {
    let home = Scratch::new("cli-prune-plan");
    october_spend(&home, "plan");
    let owned = isolated_env(home.path());
    let env: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let out = census(
        home.path(),
        home.path(),
        &["--json", "prune", "--before", "2026-10-10"],
        "",
        &env,
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(removed(&out), 2);
    assert!(!text(&out.stderr).contains("budget mode"));
}
