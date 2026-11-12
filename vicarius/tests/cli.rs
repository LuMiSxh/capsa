// SPDX-License-Identifier: MPL-2.0
//! Runs the vicarius binary with its config directory under a scratch home.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};
use vicarius::config::{Config, Effort};

/// Where vicarius reads its config under a scratch home: macOS ignores `XDG_CONFIG_HOME`.
const CONFIG_FILE: &str = if cfg!(target_os = "macos") {
    "Library/Application Support/vicarius/config.toml"
} else {
    "config/vicarius/config.toml"
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
        .env("VICARIUS_LAUNCHCTL", "/usr/bin/false")
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .expect("start vicarius")
}

#[expect(
    clippy::expect_used,
    reason = "a config that was not written must fail the test"
)]
fn saved(home: &Path) -> Config {
    Config::load_from(&home.join(CONFIG_FILE)).expect("config file")
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
    assert!(stdout.contains("senior-worker"), "{stdout}");
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

#[cfg(target_os = "macos")]
#[test]
fn a_port_change_calls_the_launchctl_named_in_the_environment() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().expect("scratch home");
    let calls = home.path().join("calls");
    let stub = home.path().join("launchctl");
    std::fs::write(
        &stub,
        format!("#!/bin/sh\necho \"$*\" >> '{}'\nexit 1\n", calls.display()),
    )
    .expect("write stub");
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
    let out = Command::new(env!("CARGO_BIN_EXE_vicarius"))
        .args(["self", "config", "set", "port", "4001"])
        .env("HOME", home.path())
        .env("VICARIUS_LAUNCHCTL", &stub)
        .output()
        .expect("start vicarius");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let seen = std::fs::read_to_string(&calls).expect("the stub was called");
    assert!(seen.contains("io.github.lumisxh.vicarius"), "{seen}");
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
    assert_eq!(config.port, 4000);
    assert_eq!(config.min_balance_usd, 2.5);
    assert_eq!(config.balance_refresh_minutes, 5);
    assert_eq!(config.max_context_tokens, 500_000);
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
    assert_eq!(saved(home.path()).agents.len(), 2);

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
    assert_eq!(config.agents.len(), 1);
    assert_eq!(config.agents[0].from, "consultant");
}

#[test]
fn agent_add_saves_custom_effort_and_defaults_to_keep() {
    let home = tempfile::tempdir().expect("scratch home");
    let default = vicarius(
        home.path(),
        &[
            "self",
            "config",
            "agent",
            "add",
            "worker",
            "--model",
            "deepseek-flash",
        ],
    );
    assert_eq!(default.status.code(), Some(0), "{}", text(&default.stderr));
    assert_eq!(saved(home.path()).agents[1].effort, Effort::Keep);

    let custom = vicarius(
        home.path(),
        &[
            "self",
            "config",
            "agent",
            "add",
            "reviewer",
            "--model",
            "deepseek-flash",
            "--effort",
            "high",
        ],
    );
    assert_eq!(custom.status.code(), Some(0), "{}", text(&custom.stderr));
    assert_eq!(saved(home.path()).agents[2].effort, Effort::High);

    let maximum = vicarius(
        home.path(),
        &[
            "self",
            "config",
            "agent",
            "add",
            "max-worker",
            "--model",
            "deepseek-flash",
            "--effort",
            "max",
        ],
    );
    assert_eq!(maximum.status.code(), Some(0), "{}", text(&maximum.stderr));
    assert_eq!(saved(home.path()).agents[3].effort, Effort::Max);

    for unsupported in ["medium", "xhigh", "extreme"] {
        let invalid = vicarius(
            home.path(),
            &[
                "self",
                "config",
                "agent",
                "add",
                "another",
                "--model",
                "deepseek-flash",
                "--effort",
                unsupported,
            ],
        );
        assert_eq!(invalid.status.code(), Some(2), "{unsupported}");
    }
}

#[test]
fn an_unknown_model_is_saved_with_a_warning() {
    let home = tempfile::tempdir().expect("scratch home");
    let out = vicarius(
        home.path(),
        &[
            "self",
            "config",
            "agent",
            "add",
            "critic",
            "--model",
            "deepseek-next",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("unknown model id"), "{stdout}");
    assert!(stdout.contains("deepseek-flash"), "{stdout}");
    assert_eq!(saved(home.path()).agents[1].model, "deepseek-next");
}

#[test]
fn agent_suspend_and_activate_change_the_flag_and_refuse_unknown_names() {
    let home = tempfile::tempdir().expect("scratch home");
    let suspended = vicarius(
        home.path(),
        &["self", "config", "agent", "suspend", "senior-worker"],
    );
    assert_eq!(
        suspended.status.code(),
        Some(0),
        "{}",
        text(&suspended.stderr)
    );
    assert!(saved(home.path()).agents[0].suspended);
    assert!(
        text(&suspended.stdout).contains("suspended"),
        "{}",
        text(&suspended.stdout)
    );

    let unknown = vicarius(
        home.path(),
        &["self", "config", "agent", "suspend", "nobody"],
    );
    assert_eq!(unknown.status.code(), Some(2));
    assert!(
        text(&unknown.stderr).contains("senior-worker"),
        "{}",
        text(&unknown.stderr)
    );

    let activated = vicarius(
        home.path(),
        &["self", "config", "agent", "activate", "--all"],
    );
    assert_eq!(
        activated.status.code(),
        Some(0),
        "{}",
        text(&activated.stderr)
    );
    assert!(!saved(home.path()).agents[0].suspended);
}

#[test]
fn uninstall_dry_run_prints_one_json_value_and_removes_nothing() {
    let home = tempfile::tempdir().expect("scratch home");
    let out = vicarius(home.path(), &["self", "uninstall", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let _: Value = serde_json::from_slice(&out.stdout).expect("one JSON value");
    assert!(Path::new(env!("CARGO_BIN_EXE_vicarius")).exists());
}
