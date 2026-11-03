// SPDX-License-Identifier: MPL-2.0
//! Runs the vicarius binary with its config directory under a scratch home.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

/// Where vicarius reads its config under a scratch home: macOS ignores `XDG_CONFIG_HOME`.
const CONFIG_FILE: &str = if cfg!(target_os = "macos") {
    "Library/Application Support/vicarius/config.json"
} else {
    "config/vicarius/config.json"
};

/// Runs vicarius with `home` as the user profile, so that its config goes under `home`.
#[expect(
    clippy::expect_used,
    reason = "a vicarius binary that cannot run must fail the test"
)]
fn vicarius(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vicarius"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("config"))
        .output()
        .expect("start vicarius")
}

#[expect(
    clippy::expect_used,
    reason = "a config that was not written must fail the test"
)]
fn saved(home: &Path) -> Value {
    let text = std::fs::read_to_string(home.join(CONFIG_FILE)).expect("config file");
    serde_json::from_str(&text).expect("config json")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn show_prints_the_defaults_and_writes_nothing() {
    let home = tempfile::tempdir().expect("scratch home");
    let out = vicarius(home.path(), &["self", "config"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("3337"), "{stdout}");
    assert!(
        stdout.contains("senior-worker-ds on deepseek-flash"),
        "{stdout}"
    );
    assert!(!home.path().join(CONFIG_FILE).exists());
}

#[test]
fn json_is_one_value_with_the_settings_and_the_path() {
    let home = tempfile::tempdir().expect("scratch home");
    let out = vicarius(home.path(), &["--json", "self", "config"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).expect("one JSON value");
    assert_eq!(value["port"], 3337);
    assert_eq!(value["agents"][0]["from"], "senior-worker");
    assert!(value["path"].is_string());
}

#[test]
fn set_saves_each_scalar_and_prints_the_settings() {
    let home = tempfile::tempdir().expect("scratch home");
    let out = vicarius(home.path(), &["self", "config", "set", "port", "4000"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("4000"), "{}", text(&out.stdout));
    for (key, value) in [
        ("min-balance", "2.5"),
        ("balance-refresh", "5"),
        ("max-context", "500000"),
    ] {
        let out = vicarius(home.path(), &["self", "config", "set", key, value]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let config = saved(home.path());
    assert_eq!(config["port"], 4000);
    assert_eq!(config["min_balance_usd"], 2.5);
    assert_eq!(config["balance_refresh_minutes"], 5);
    assert_eq!(config["max_context_tokens"], 500_000);
}

#[test]
fn set_refuses_a_value_that_breaks_its_rule_and_writes_nothing() {
    let home = tempfile::tempdir().expect("scratch home");
    let refused = [
        ("port", "0"),
        ("port", "not-a-port"),
        ("max-context", "999"),
        ("min-balance", "-1"),
    ];
    for (key, value) in refused {
        let out = vicarius(home.path(), &["self", "config", "set", key, value]);
        assert_eq!(out.status.code(), Some(2), "{key} {value}");
        assert!(
            text(&out.stderr).contains("error:"),
            "{}",
            text(&out.stderr)
        );
        assert!(text(&out.stderr).contains("hint:"), "{}", text(&out.stderr));
    }
    assert!(!home.path().join(CONFIG_FILE).exists());
}

#[test]
fn agent_add_and_remove_change_the_list_and_refuse_bad_ones() {
    let home = tempfile::tempdir().expect("scratch home");
    let out = vicarius(
        home.path(),
        &[
            "self",
            "config",
            "agent",
            "add",
            "consultant",
            "--model",
            "deepseek-v4-pro",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        saved(home.path())["agents"].as_array().map(Vec::len),
        Some(2)
    );

    let taken = vicarius(
        home.path(),
        &[
            "self",
            "config",
            "agent",
            "add",
            "senior-worker",
            "--model",
            "deepseek-flash",
        ],
    );
    assert_eq!(taken.status.code(), Some(2));
    assert!(
        text(&taken.stderr).contains("agent remove senior-worker"),
        "{}",
        text(&taken.stderr)
    );

    let bad_name = vicarius(
        home.path(),
        &[
            "self",
            "config",
            "agent",
            "add",
            "a.b",
            "--model",
            "deepseek-flash",
        ],
    );
    assert_eq!(bad_name.status.code(), Some(2));

    let bad_model = vicarius(
        home.path(),
        &[
            "self", "config", "agent", "add", "critic", "--model", "gpt-5",
        ],
    );
    assert_eq!(bad_model.status.code(), Some(2));
    assert!(
        text(&bad_model.stderr).contains("deepseek-"),
        "{}",
        text(&bad_model.stderr)
    );

    let no_model = vicarius(home.path(), &["self", "config", "agent", "add", "critic"]);
    assert_eq!(no_model.status.code(), Some(2));

    let unknown = vicarius(
        home.path(),
        &["self", "config", "agent", "remove", "nobody"],
    );
    assert_eq!(unknown.status.code(), Some(2));
    assert!(
        text(&unknown.stderr).contains("consultant"),
        "{}",
        text(&unknown.stderr)
    );

    let removed = vicarius(
        home.path(),
        &["self", "config", "agent", "remove", "senior-worker"],
    );
    assert_eq!(removed.status.code(), Some(0), "{}", text(&removed.stderr));
    let config = saved(home.path());
    assert_eq!(config["agents"].as_array().map(Vec::len), Some(1));
    assert_eq!(config["agents"][0]["from"], "consultant");
}

#[test]
fn uninstall_dry_run_prints_one_json_value_and_removes_nothing() {
    let home = tempfile::tempdir().expect("scratch home");
    let out = vicarius(home.path(), &["self", "uninstall", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let _: Value = serde_json::from_slice(&out.stdout).expect("one JSON value");
    assert!(Path::new(env!("CARGO_BIN_EXE_vicarius")).exists());
}
