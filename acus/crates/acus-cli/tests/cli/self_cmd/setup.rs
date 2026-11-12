// SPDX-License-Identifier: MPL-2.0

use super::{json_line, sandboxed};
use crate::common::acus_env;
#[cfg(feature = "cmd-update")]
use crate::common::{decode, output_retrying_busy};

#[test]
fn setup_dry_run_then_setup_twice_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    let env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap());

    let (code, out, err) = acus_env(&["self", "setup", "--dry-run", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    let v = json_line(&out);
    assert_eq!(v["dry_run"], true);
    let checks = v["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 4, "{out}");
    assert!(
        checks[1]["detail"]
            .as_str()
            .unwrap()
            .starts_with("would install")
    );
    assert!(!claude.join("settings.json").exists());
    assert!(!home.join(".claude/skills/acus").exists());

    let (code, out, err) = acus_env(&["self", "setup", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    let checks = json_line(&out)["checks"].as_array().unwrap().clone();
    assert!(checks.iter().all(|c| c["status"] == "ok"), "{out}");
    assert!(
        checks[0]["detail"]
            .as_str()
            .unwrap()
            .starts_with("installed")
    );
    assert!(
        checks[1]["detail"]
            .as_str()
            .unwrap()
            .starts_with("installed")
    );
    assert!(claude.join("settings.json").is_file());
    assert!(home.join(".claude/skills/acus/SKILL.md").is_file());

    let (code, out, err) = acus_env(&["self", "setup", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    for check in json_line(&out)["checks"].as_array().unwrap() {
        let detail = check["detail"].as_str().unwrap();
        assert!(detail.starts_with("already set"), "{out}");
    }
}

#[cfg(feature = "cmd-update")]
#[test]
fn setup_installs_the_reference_files_and_uninstall_removes_them() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    let copy = dir.path().join("acus");
    std::fs::copy(env!("CARGO_BIN_EXE_acus"), &copy).unwrap();
    let run = |args: &[&str]| {
        let mut command = std::process::Command::new(&copy);
        command
            .args(args)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("CLAUDE_CONFIG_DIR", &claude)
            .env("ACUS_CONFIG", "/nonexistent/acus.toml");
        decode(output_retrying_busy(&mut command).unwrap())
    };

    let (code, _, err) = run(&["self", "setup"]);
    assert_eq!(code, 0, "{err}");
    let skill = home.join(".claude/skills/acus");
    for topic in ["find", "read", "patch", "git", "ctx"] {
        let file = skill.join("reference").join(format!("{topic}.md"));
        assert!(file.is_file(), "{}", file.display());
    }

    let (code, out, err) = run(&["self", "uninstall"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("removed"), "{out}");
    assert!(
        !skill.exists(),
        "the reference directory is gone with the skill"
    );
}

#[test]
fn setup_replaces_the_old_hook_entries() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir(&home).unwrap();
    let old = r#"{"hooks":{
        "SessionStart":[{"matcher":"startup|clear|compact",
            "hooks":[{"type":"command","command":"acus self skill 2>/dev/null || true"}]}],
        "PreToolUse":[{"matcher":"Bash|Read|Grep|Glob|PowerShell",
            "hooks":[{"type":"command","command":"acus guard 2>/dev/null || true"}]}]
    }}"#;
    std::fs::write(claude.join("settings.json"), old).unwrap();
    let env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap());

    let (code, _, err) = acus_env(&["self", "setup", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    let text = std::fs::read_to_string(claude.join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let commands: Vec<&str> = v["hooks"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|groups| groups.as_array().unwrap())
        .flat_map(|group| group["hooks"].as_array().unwrap())
        .filter_map(|hook| hook["command"].as_str())
        .collect();
    assert_eq!(commands.len(), 3, "{text}");
    assert!(
        commands
            .iter()
            .all(|command| *command == "acus hook 2>/dev/null || true"),
        "{text}"
    );
}

#[test]
fn setup_registers_subagent_start_for_every_subagent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    let env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap());

    let (code, _, err) = acus_env(&["self", "setup"], &env);
    assert_eq!(code, 0, "{err}");
    let text = std::fs::read_to_string(claude.join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let groups = v["hooks"]["SubagentStart"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{text}");
    assert!(groups[0].get("matcher").is_none(), "{text}");
    assert_eq!(
        groups[0]["hooks"][0]["command"], "acus hook 2>/dev/null || true",
        "{text}"
    );
}

#[test]
fn setup_keeps_a_subagent_start_entry_with_a_custom_matcher() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir(&home).unwrap();
    let scoped = r#"{"hooks":{"SubagentStart":[{"matcher":"Explore|reviewer",
        "hooks":[{"type":"command","command":"acus hook 2>/dev/null || true"}]}]}}"#;
    std::fs::write(claude.join("settings.json"), scoped).unwrap();
    let env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap());

    let (code, out, err) = acus_env(&["self", "setup", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    let v = json_line(&out);
    let hook = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "hook SubagentStart")
        .unwrap();
    assert!(
        hook["detail"].as_str().unwrap().starts_with("already set"),
        "{hook}"
    );
    let text = std::fs::read_to_string(claude.join("settings.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&text).unwrap();
    let want: serde_json::Value = serde_json::from_str(scoped).unwrap();
    assert_eq!(
        settings["hooks"]["SubagentStart"], want["hooks"]["SubagentStart"],
        "{text}"
    );
}

#[test]
fn setup_run_twice_writes_the_same_settings() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    let env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap());
    assert_eq!(acus_env(&["self", "setup"], &env).0, 0);
    let first = std::fs::read_to_string(claude.join("settings.json")).unwrap();

    let (code, out, err) = acus_env(&["self", "setup", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        std::fs::read_to_string(claude.join("settings.json")).unwrap(),
        first
    );
    let v = json_line(&out);
    let hooks: Vec<_> = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"].as_str().is_some_and(|n| n.starts_with("hook ")))
        .collect();
    assert_eq!(hooks.len(), 3, "{out}");
    for hook in hooks {
        assert!(
            hook["detail"].as_str().unwrap().starts_with("already set"),
            "{hook}"
        );
    }
}

#[test]
fn setup_add_missing_adds_only_the_hooks_that_are_missing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    let env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap());
    assert_eq!(acus_env(&["self", "setup"], &env).0, 0);
    let path = claude.join("settings.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["hooks"]["PreToolUse"][0]["matcher"] = "Bash".into();
    v["hooks"].as_object_mut().unwrap().remove("SubagentStart");
    std::fs::write(&path, v.to_string()).unwrap();

    let (code, out, err) = acus_env(&["self", "setup", "--add-missing", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    let report = json_line(&out);
    let names: Vec<_> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["hook SubagentStart"], "{out}");
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after["hooks"]["PreToolUse"][0]["matcher"], "Bash");
    assert_eq!(after["hooks"]["SubagentStart"].as_array().unwrap().len(), 1);

    let (code, out, err) = acus_env(&["self", "setup", "--add-missing"], &env);
    assert_eq!(
        (code, out.as_str()),
        (0, ""),
        "second run prints nothing: {err}"
    );
}

#[test]
fn setup_add_missing_writes_nothing_before_a_setup() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir(&home).unwrap();
    std::fs::write(claude.join("settings.json"), "{}").unwrap();
    let env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap());

    let (code, out, err) = acus_env(&["self", "setup", "--add-missing"], &env);
    assert_eq!((code, out.as_str()), (0, ""), "{err}");
    assert_eq!(
        std::fs::read_to_string(claude.join("settings.json")).unwrap(),
        "{}"
    );
}

#[test]
fn setup_moves_a_guard_log_left_beside_the_config_once() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    let old = dir.path().join("guard.jsonl");
    std::fs::write(&old, "old\n").unwrap();
    std::fs::write(dir.path().join("guard.jsonl.old"), "older\n").unwrap();
    let state = dir.path().join("state").join("acus");
    let mut env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap()).to_vec();
    let config = dir.path().join("config.toml");
    let state_root = dir.path().join("state");
    env.push(("ACUS_CONFIG", config.to_str().unwrap()));
    env.push(("XDG_STATE_HOME", state_root.to_str().unwrap()));

    let (code, out, err) = acus_env(&["self", "setup", "--dry-run", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("would move"), "{out}");
    assert!(old.exists(), "a dry run moves nothing");

    assert_eq!(acus_env(&["self", "setup"], &env).0, 0);
    assert!(!old.exists());
    let moved = std::fs::read_to_string(state.join("guard.jsonl")).unwrap();
    assert_eq!(moved, "old\n");
    let rotated = std::fs::read_to_string(state.join("guard.jsonl.old")).unwrap();
    assert_eq!(rotated, "older\n");

    assert_eq!(acus_env(&["self", "setup"], &env).0, 0);
    let kept = std::fs::read_to_string(state.join("guard.jsonl")).unwrap();
    assert_eq!(kept, "old\n", "a second setup finds nothing to move");
}

#[test]
fn setup_merges_old_guard_log_files_in_front_of_the_state_log() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(dir.path().join("guard.jsonl.old"), "older\n").unwrap();
    std::fs::write(dir.path().join("guard.jsonl"), "old\n").unwrap();
    let state = dir.path().join("state").join("acus");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("guard.jsonl"), "current\n").unwrap();
    let mut env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap()).to_vec();
    let config = dir.path().join("config.toml");
    let state_root = dir.path().join("state");
    env.push(("ACUS_CONFIG", config.to_str().unwrap()));
    env.push(("XDG_STATE_HOME", state_root.to_str().unwrap()));

    let (code, out, err) = acus_env(&["self", "setup", "--dry-run", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("would merge"), "{out}");
    assert!(
        dir.path().join("guard.jsonl").exists(),
        "a dry run merges nothing"
    );

    let (code, out, err) = acus_env(&["self", "setup", "--json"], &env);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("merged"), "{out}");
    let merged = std::fs::read_to_string(state.join("guard.jsonl")).unwrap();
    assert_eq!(merged, "older\nold\ncurrent\n");
    assert!(!dir.path().join("guard.jsonl").exists());
    assert!(!dir.path().join("guard.jsonl.old").exists());
}

#[test]
fn setup_merge_leaves_the_state_rotated_copy_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(dir.path().join("guard.jsonl.old"), "older\n").unwrap();
    std::fs::write(dir.path().join("guard.jsonl"), "old\n").unwrap();
    let state = dir.path().join("state").join("acus");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("guard.jsonl"), "current\n").unwrap();
    std::fs::write(state.join("guard.jsonl.old"), "rotated\n").unwrap();
    let mut env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap()).to_vec();
    let config = dir.path().join("config.toml");
    let state_root = dir.path().join("state");
    env.push(("ACUS_CONFIG", config.to_str().unwrap()));
    env.push(("XDG_STATE_HOME", state_root.to_str().unwrap()));

    assert_eq!(acus_env(&["self", "setup"], &env).0, 0);
    let merged = std::fs::read_to_string(state.join("guard.jsonl")).unwrap();
    assert_eq!(merged, "older\nold\ncurrent\n");
    let rotated = std::fs::read_to_string(state.join("guard.jsonl.old")).unwrap();
    assert_eq!(rotated, "rotated\n");
    assert!(!dir.path().join("guard.jsonl").exists());
    assert!(!dir.path().join("guard.jsonl.old").exists());
}
