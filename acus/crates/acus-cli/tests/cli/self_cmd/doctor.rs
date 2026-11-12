// SPDX-License-Identifier: MPL-2.0

use super::{json_line, sandboxed};
use crate::common::acus_env;

#[test]
fn doctor_json_is_healthy_after_setup_and_fails_on_a_broken_config() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    let h = home.to_str().unwrap();
    let c = claude.to_str().unwrap();
    let env = sandboxed(h, c);
    assert_eq!(acus_env(&["self", "setup"], &env).0, 0);

    let (code, out, err) = acus_env(&["self", "doctor", "--json"], &env);
    assert_eq!(code, 0, "{err}\n{out}");
    let v = json_line(&out);
    assert_eq!(v["ok"], true);
    let names: Vec<&str> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["install", "config", "skill", "hooks", "guard"]);
    let status = |name: &str| {
        v["groups"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["checks"].as_array().unwrap())
            .find(|c| c["name"] == name)
            .map(|c| c["status"].clone())
    };
    assert_eq!(status("claude skill").unwrap(), "ok");
    assert_eq!(status("SessionStart hook").unwrap(), "ok");
    assert_eq!(status("PreToolUse hook").unwrap(), "ok");
    assert_eq!(status("SubagentStart hook").unwrap(), "ok");

    let bad = dir.path().join("acus.toml");
    std::fs::write(&bad, "[commands\ndisabled = 1\n").unwrap();
    let broken = [
        ("HOME", h),
        ("USERPROFILE", h),
        ("CLAUDE_CONFIG_DIR", c),
        ("ACUS_CONFIG", bad.to_str().unwrap()),
    ];
    let (code, out, _) = acus_env(&["self", "doctor", "--json"], &broken);
    assert_eq!(code, 1, "{out}");
    let broken_v = json_line(&out);
    assert_eq!(broken_v["ok"], false);
    let config = broken_v["groups"][1]["checks"][0].clone();
    assert_eq!(config["name"], "config file");
    assert_eq!(config["status"], "fail");
}

#[test]
fn doctor_warns_when_the_hook_matcher_misses_a_tool() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir(&home).unwrap();
    let hooks = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"acus hook 2>/dev/null || true"}]}]}}"#;
    std::fs::write(claude.join("settings.json"), hooks).unwrap();
    let env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap());

    let (code, out, _) = acus_env(&["self", "doctor", "--json"], &env);
    assert_eq!(code, 0, "a warning is not a failure: {out}");
    let v = json_line(&out);
    let guard = v["groups"][3]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "PreToolUse hook")
        .unwrap();
    assert_eq!(guard["status"], "warn");
    assert!(
        guard["detail"].as_str().unwrap().contains("Grep"),
        "{guard}"
    );
}

#[test]
fn doctor_flags_the_old_hook_entries() {
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

    let (code, out, err) = acus_env(&["self", "doctor", "--json"], &env);
    assert_eq!(code, 0, "a warning is not a failure: {err}\n{out}");
    let v = json_line(&out);
    for name in ["SessionStart hook", "PreToolUse hook"] {
        let check = v["groups"][3]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == name)
            .unwrap();
        assert_eq!(check["status"], "warn", "{check}");
        assert!(
            check["detail"].as_str().unwrap().contains("outdated"),
            "{check}"
        );
        assert!(
            check["hint"].as_str().unwrap().contains("self setup"),
            "{check}"
        );
    }
}

#[test]
fn doctor_warns_when_subagent_start_is_not_registered() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir(&home).unwrap();
    std::fs::write(claude.join("settings.json"), "{}").unwrap();
    let env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap());

    let (code, out, _) = acus_env(&["self", "doctor", "--json"], &env);
    assert_eq!(code, 0, "a warning is not a failure: {out}");
    let v = json_line(&out);
    let subagent = v["groups"][3]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "SubagentStart hook")
        .unwrap();
    assert_eq!(subagent["status"], "warn", "{subagent}");
    assert!(
        subagent["hint"].as_str().unwrap().contains("self setup"),
        "{subagent}"
    );
}

#[cfg(unix)]
#[test]
fn doctor_fails_when_the_guard_log_is_read_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir(&home).unwrap();
    std::fs::write(claude.join("settings.json"), "{}").unwrap();
    let state = dir.path().join("state").join("acus");
    std::fs::create_dir_all(&state).unwrap();
    let log = state.join("guard.jsonl");
    std::fs::write(&log, "").unwrap();
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o444)).unwrap();
    // Root ignores the mode bits, so the log stays writable and there is nothing to test.
    if std::fs::OpenOptions::new().append(true).open(&log).is_ok() {
        return;
    }
    let mut env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap()).to_vec();
    let config = dir.path().join("config.toml");
    env.push(("ACUS_CONFIG", config.to_str().unwrap()));
    let state_root = dir.path().join("state");
    env.push(("XDG_STATE_HOME", state_root.to_str().unwrap()));

    let (_, out, _) = acus_env(&["self", "doctor", "--json"], &env);
    let v = json_line(&out);
    let check = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["checks"].as_array().unwrap())
        .find(|c| c["name"] == "guard log file")
        .unwrap();
    assert_eq!(check["status"], "fail", "{check}");
}

#[test]
fn doctor_warns_about_a_guard_log_left_beside_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(dir.path().join("guard.jsonl"), "").unwrap();
    let mut env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap()).to_vec();
    let config = dir.path().join("config.toml");
    let state_root = dir.path().join("state");
    env.push(("ACUS_CONFIG", config.to_str().unwrap()));
    env.push(("XDG_STATE_HOME", state_root.to_str().unwrap()));

    let (_, out, _) = acus_env(&["self", "doctor", "--json"], &env);
    let v = json_line(&out);
    let check = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["checks"].as_array().unwrap())
        .find(|c| c["name"] == "guard log location")
        .unwrap();
    assert_eq!(check["status"], "warn", "{check}");
    assert_eq!(check["hint"], "run acus self setup", "{check}");
}

#[test]
fn doctor_warns_about_guard_log_files_left_beside_a_state_log() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(dir.path().join("guard.jsonl"), "").unwrap();
    std::fs::write(dir.path().join("guard.jsonl.old"), "").unwrap();
    let state = dir.path().join("state").join("acus");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("guard.jsonl"), "").unwrap();
    let mut env = sandboxed(home.to_str().unwrap(), claude.to_str().unwrap()).to_vec();
    let config = dir.path().join("config.toml");
    let state_root = dir.path().join("state");
    env.push(("ACUS_CONFIG", config.to_str().unwrap()));
    env.push(("XDG_STATE_HOME", state_root.to_str().unwrap()));

    let (_, out, _) = acus_env(&["self", "doctor", "--json"], &env);
    let v = json_line(&out);
    let check = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["checks"].as_array().unwrap())
        .find(|c| c["name"] == "guard log location")
        .unwrap();
    assert_eq!(check["status"], "warn", "{check}");
    assert_eq!(check["hint"], "run acus self setup", "{check}");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("guard.jsonl.old"),
        "{check}"
    );
}
