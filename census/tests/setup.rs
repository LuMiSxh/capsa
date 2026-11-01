// SPDX-License-Identifier: MPL-2.0
//! Runs `census self setup` as a binary under a scratch home: the Claude configuration is at
//! `home/claude` and the skill goes to `home/.claude/skills`.

mod common;

use common::Scratch;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SKILL: &str = include_str!("../skill/SKILL.md");
const ITEMS: [&str; 4] = [
    "skill",
    "statusLine",
    "hook UserPromptSubmit",
    "hook PostToolUse",
];
/// The entries MacSet writes: a shell line that runs census first, with a fallback after it.
const MACSET_HOOK: &str = r#"census hook 2>/dev/null || "$HOME/.cargo/bin/census" hook || true"#;
const MACSET_LINE: &str = "census statusline --line";

/// Runs `census self setup` with `args` under the scratch home.
fn setup(home: &Scratch, args: &[&str]) -> Output {
    setup_with(Path::new(env!("CARGO_BIN_EXE_census")), home, args)
}

/// Runs the binary at `exe` as `census self setup` with `args` under the scratch home.
#[expect(
    clippy::expect_used,
    reason = "a census binary that cannot run must fail the test"
)]
fn setup_with(exe: &Path, home: &Scratch, args: &[&str]) -> Output {
    let mut command = Command::new(exe);
    command
        .args(["self", "setup"])
        .args(args)
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path().join("claude"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env_remove("CAPSA_HUMAN");
    output_retrying_busy(&mut command).expect("run census self setup")
}

/// Runs `cmd` to completion. Linux reports a just-copied binary as busy (ETXTBSY) while another
/// test thread's fork still holds the copy's write descriptor, so the spawn is retried briefly.
fn output_retrying_busy(cmd: &mut Command) -> std::io::Result<Output> {
    for _ in 0..50 {
        match cmd.output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            other => return other,
        }
    }
    cmd.output()
}

fn settings_path(home: &Scratch) -> PathBuf {
    home.path().join("claude/settings.json")
}

fn backup_path(home: &Scratch) -> PathBuf {
    home.path().join("claude/settings.json.bak")
}

fn skill_path(home: &Scratch) -> PathBuf {
    home.path().join(".claude/skills/census/SKILL.md")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[expect(
    clippy::expect_used,
    reason = "a fixture that cannot be read must fail the test"
)]
fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read the file")
}

#[expect(
    clippy::expect_used,
    reason = "a settings file that is not JSON must fail the test"
)]
fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read(path)).expect("the settings are JSON")
}

/// The status the report gives `item`, without its padding; empty when there is no such line.
fn status_of(report: &str, item: &str) -> String {
    report
        .lines()
        .find_map(|line| line.strip_prefix(item))
        .map(str::trim)
        .unwrap_or_default()
        .to_owned()
}

/// Whether `command` is this binary followed by `args`, such as " hook".
#[expect(
    clippy::expect_used,
    reason = "the census binary must exist for the test to mean anything"
)]
fn runs_census(command: &str, args: &str) -> bool {
    let exe = Path::new(env!("CARGO_BIN_EXE_census"))
        .canonicalize()
        .expect("canonicalize the census binary");
    command
        .strip_suffix(args)
        .and_then(|path| Path::new(path).canonicalize().ok())
        .is_some_and(|path| path == exe)
}

#[expect(
    clippy::expect_used,
    reason = "a settings file without a census hook must fail the test"
)]
fn hook_command(settings: &Value, event: &str) -> String {
    settings["hooks"][event][0]["hooks"][0]["command"]
        .as_str()
        .expect("the event has a hook command")
        .to_owned()
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

#[test]
fn missing_settings_get_every_item_and_valid_json() {
    let home = Scratch::new("setup-missing");
    let out = setup(&home, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let report = stdout(&out);
    for item in ITEMS {
        assert_eq!(status_of(&report, item), "installed", "{report}");
    }
    let settings = read_json(&settings_path(&home));
    assert_eq!(settings["statusLine"]["type"], "command");
    assert!(runs_census(
        settings["statusLine"]["command"]
            .as_str()
            .unwrap_or_default(),
        " statusline --line"
    ));
    assert!(runs_census(
        &hook_command(&settings, "UserPromptSubmit"),
        " hook"
    ));
    assert!(runs_census(
        &hook_command(&settings, "PostToolUse"),
        " hook"
    ));
    assert_eq!(read(&skill_path(&home)), SKILL);
}

#[test]
fn a_second_run_changes_nothing_and_writes_no_backup() {
    let home = Scratch::new("setup-again");
    home.write("claude/settings.json", r#"{"theme":"dark"}"#);
    assert_eq!(setup(&home, &[]).status.code(), Some(0));
    assert_eq!(read(&backup_path(&home)), r#"{"theme":"dark"}"#);
    let before = read(&settings_path(&home));
    std::fs::remove_file(backup_path(&home)).expect("remove the backup");

    let out = setup(&home, &[]);
    assert_eq!(out.status.code(), Some(0));
    let report = stdout(&out);
    for item in ITEMS {
        assert_eq!(status_of(&report, item), "already set", "{report}");
    }
    assert_eq!(read(&settings_path(&home)), before);
    assert!(!backup_path(&home).exists());
}

#[test]
fn entries_that_already_run_census_are_left_alone() {
    let home = Scratch::new("setup-macset");
    let original = json!({
        "statusLine": {"type": "command", "command": MACSET_LINE},
        "hooks": {
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": MACSET_HOOK}]}],
            "PostToolUse": [{"hooks": [{"type": "command", "command": MACSET_HOOK}]}]
        }
    })
    .to_string();
    home.write("claude/settings.json", &original);

    let out = setup(&home, &[]);
    assert_eq!(out.status.code(), Some(0));
    let report = stdout(&out);
    // This home has no skill yet, so the skill is the one item that is installed.
    assert_eq!(status_of(&report, "skill"), "installed", "{report}");
    for item in &ITEMS[1..] {
        assert_eq!(status_of(&report, item), "already set", "{report}");
    }
    assert_eq!(read(&settings_path(&home)), original);
    assert!(!backup_path(&home).exists());
}

#[test]
fn a_foreign_status_line_stays_without_force_and_is_replaced_with_it() {
    let home = Scratch::new("setup-foreign");
    home.write(
        "claude/settings.json",
        r#"{"statusLine":{"type":"command","command":"my-line"}}"#,
    );

    let out = setup(&home, &[]);
    assert_eq!(out.status.code(), Some(0));
    let report = stdout(&out);
    assert_eq!(
        status_of(&report, "statusLine"),
        "skipped (foreign statusLine; --force replaces it)",
        "{report}"
    );
    assert_eq!(
        status_of(&report, "hook PostToolUse"),
        "installed",
        "{report}"
    );
    let settings = read_json(&settings_path(&home));
    assert_eq!(settings["statusLine"]["command"], "my-line");

    let out = setup(&home, &["--force"]);
    assert_eq!(out.status.code(), Some(0));
    let report = stdout(&out);
    assert_eq!(
        status_of(&report, "statusLine"),
        "installed (replaced foreign statusLine)",
        "{report}"
    );
    let settings = read_json(&settings_path(&home));
    assert!(runs_census(
        settings["statusLine"]["command"]
            .as_str()
            .unwrap_or_default(),
        " statusline --line"
    ));
}

#[test]
fn unrelated_keys_and_hooks_keep_their_order() {
    let home = Scratch::new("setup-order");
    let original = r#"{"theme":"dark","hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify"}]}],"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"lint"}]}]},"env":{"A":"1"}}"#;
    home.write("claude/settings.json", original);

    let out = setup(&home, &[]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(read(&backup_path(&home)), original);
    let settings = read_json(&settings_path(&home));
    assert_eq!(keys(&settings), ["theme", "hooks", "env", "statusLine"]);
    assert_eq!(
        keys(&settings["hooks"]),
        ["Stop", "PostToolUse", "UserPromptSubmit"]
    );
    assert_eq!(
        settings["hooks"]["Stop"],
        json!([{"hooks": [{"type": "command", "command": "notify"}]}])
    );
    assert_eq!(
        settings["hooks"]["PostToolUse"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(
        settings["hooks"]["PostToolUse"][0],
        json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "lint"}]})
    );
    assert_eq!(settings["env"], json!({"A": "1"}));
}

#[test]
fn invalid_json_exits_2_and_nothing_is_written() {
    let home = Scratch::new("setup-invalid");
    home.write("claude/settings.json", "{ not json");

    let out = setup(&home, &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("error:") && err.contains("hint:"), "{err}");
    assert_eq!(read(&settings_path(&home)), "{ not json");
    assert!(!skill_path(&home).exists());
}

#[test]
fn hooks_or_an_event_of_the_wrong_shape_exit_2_and_nothing_is_written() {
    for original in [r#"{"hooks":[]}"#, r#"{"hooks":{"PostToolUse":{}}}"#] {
        let home = Scratch::new("setup-shape");
        home.write("claude/settings.json", original);

        let out = setup(&home, &[]);
        assert_eq!(out.status.code(), Some(2), "{original}");
        assert_eq!(read(&settings_path(&home)), original);
        assert!(!skill_path(&home).exists());
    }
}

#[test]
fn dry_run_reports_and_writes_nothing() {
    let home = Scratch::new("setup-dry");
    let out = setup(&home, &["--dry-run"]);
    assert_eq!(out.status.code(), Some(0));
    let report = stdout(&out);
    for item in ITEMS {
        assert_eq!(status_of(&report, item), "would install", "{report}");
    }
    assert!(report.contains("dry run"), "{report}");
    assert!(!home.path().join("claude").exists());
    assert!(!skill_path(&home).exists());

    home.write("claude/settings.json", r#"{"theme":"dark"}"#);
    let out = setup(&home, &["--dry-run"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(read(&settings_path(&home)), r#"{"theme":"dark"}"#);
    assert!(!backup_path(&home).exists());
}

#[test]
fn json_output_has_the_same_items() {
    let home = Scratch::new("setup-json");
    let out = setup(&home, &["--json"]);
    assert_eq!(out.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&out.stdout).expect("the output is JSON");
    assert_eq!(value["dry_run"], false);
    assert_eq!(
        value["items"],
        json!([
            {"name": "skill", "status": "installed"},
            {"name": "statusLine", "status": "installed"},
            {"name": "hook UserPromptSubmit", "status": "installed"},
            {"name": "hook PostToolUse", "status": "installed"}
        ])
    );
}

#[test]
fn a_binary_in_a_path_with_a_space_is_recognised_on_the_next_run() {
    let home = Scratch::new("setup-spaced");
    std::fs::create_dir_all(home.path().join("My Apps")).expect("create the directory");
    let exe = home.path().join("My Apps/census");
    std::fs::copy(env!("CARGO_BIN_EXE_census"), &exe).expect("copy the binary");

    let out = setup_with(&exe, &home, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let settings = read_json(&settings_path(&home));
    let line = settings["statusLine"]["command"]
        .as_str()
        .unwrap_or_default();
    assert!(
        line.starts_with('"') && line.ends_with("\" statusline --line"),
        "{line}"
    );
    let first = read(&settings_path(&home));

    let out = setup_with(&exe, &home, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let report = stdout(&out);
    for item in ITEMS {
        assert_eq!(status_of(&report, item), "already set", "{report}");
    }
    assert_eq!(read(&settings_path(&home)), first);
}

#[cfg(unix)]
#[test]
fn a_symlinked_settings_file_stays_a_link_and_its_target_is_written() {
    let home = Scratch::new("setup-link");
    let target = home.write("dotfiles/settings.json", r#"{"theme":"dark"}"#);
    std::fs::create_dir_all(home.path().join("claude")).expect("create the config directory");
    std::os::unix::fs::symlink(&target, settings_path(&home)).expect("link the settings");

    let out = setup(&home, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let link = settings_path(&home);
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("stat the link")
            .file_type()
            .is_symlink()
    );
    let written = read(&target);
    assert!(
        written.contains("statusLine") && written.contains("\"theme\""),
        "{written}"
    );
    assert_eq!(read(&link), written);
}

#[cfg(unix)]
#[test]
fn the_mode_of_the_settings_file_is_kept() {
    use std::os::unix::fs::PermissionsExt;

    let home = Scratch::new("setup-mode");
    let path = home.write("claude/settings.json", r#"{"theme":"dark"}"#);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("set the mode");

    let out = setup(&home, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let mode = std::fs::metadata(&path)
        .expect("stat the settings")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}
