// SPDX-License-Identifier: MPL-2.0

#[cfg(feature = "cmd-update")]
use super::common::output_retrying_busy;
use super::common::{acus, acus_env, decode};

#[test]
fn the_old_top_level_names_are_gone() {
    for name in ["skill", "update"] {
        let (code, _, err) = acus(&[name]);
        assert_eq!(code, 2, "{name}: {err}");
        assert!(err.contains("unrecognized subcommand"), "{name}: {err}");
    }
}

#[test]
fn self_lists_update_skill_and_uninstall() {
    let (code, out, _) = acus(&["self", "--help"]);
    assert_eq!(code, 0);
    for name in ["update", "setup", "doctor", "skill", "uninstall"] {
        assert!(out.contains(name), "{name}: {out}");
    }
}

#[test]
fn a_disabled_name_still_blocks_its_self_action() {
    let (code, _, err) = acus_env(&["self", "skill"], &[("ACUS_DISABLE", "skill")]);
    assert_eq!(code, 2);
    assert!(err.contains("disabled"), "{err}");
}

/// Runs acus with HOME, USERPROFILE and CLAUDE_CONFIG_DIR set, so no real user file is touched.
fn sandboxed<'a>(home: &'a str, claude: &'a str) -> [(&'a str, &'a str); 3] {
    [
        ("HOME", home),
        ("USERPROFILE", home),
        ("CLAUDE_CONFIG_DIR", claude),
    ]
}

fn json_line(out: &str) -> serde_json::Value {
    assert_eq!(out.lines().count(), 1, "not one JSON line: {out}");
    serde_json::from_str(out.trim()).unwrap()
}

#[test]
fn skill_target_selects_the_agent_dirs() {
    for (target, claude, codex) in [
        ("claude", true, false),
        ("codex", false, true),
        ("all", true, true),
    ] {
        let home = tempfile::tempdir().unwrap();
        let h = home.path().to_str().unwrap();
        let (code, _, err) = acus_env(
            &["self", "skill", "--install", "--target", target],
            &[("HOME", h), ("USERPROFILE", h)],
        );
        assert_eq!(code, 0, "{target}: {err}");
        let written = |dir: &str| home.path().join(dir).join("SKILL.md").is_file();
        assert_eq!(written(".claude/skills/acus"), claude, "{target}");
        assert_eq!(written(".agents/skills/acus"), codex, "{target}");
    }
}

#[test]
fn skill_refuses_unknown_targets_and_the_retired_flags() {
    for args in [
        &["self", "skill", "--install", "--target", "vim"][..],
        &["self", "skill", "--install", "--agent", "claude"][..],
        &["self", "skill", "--install", "--hooks"][..],
    ] {
        let (code, _, err) = acus(args);
        assert_eq!(code, 2, "{args:?}: {err}");
    }
}

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

#[cfg(feature = "cmd-update")]
#[test]
fn uninstall_json_is_one_value_and_keeps_the_skill() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path().to_str().unwrap();
    let skill = home.path().join(".claude/skills/acus");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "x").unwrap();
    let (code, out, err) = acus_env(
        &["self", "uninstall", "--dry-run", "--json"],
        &[("HOME", h), ("USERPROFILE", h)],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(json_line(&out)["removed"], false);
    assert!(skill.join("SKILL.md").is_file());
}

#[cfg(feature = "cmd-update")]
#[test]
fn uninstall_dry_run_removes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let skill = home.path().join(".claude/skills/acus");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "x").unwrap();
    let home_str = home.path().to_str().unwrap();
    let (code, out, err) = acus_env(
        &["self", "uninstall", "--dry-run"],
        &[("HOME", home_str), ("USERPROFILE", home_str)],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("would remove"), "{out}");
    assert!(out.contains("SKILL.md"), "{out}");
    assert!(skill.join("SKILL.md").is_file());
    assert!(std::path::Path::new(env!("CARGO_BIN_EXE_acus")).is_file());
}

#[cfg(feature = "cmd-update")]
#[test]
fn uninstall_removes_the_binary_and_the_skill_of_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let copy = dir.path().join("acus");
    std::fs::copy(env!("CARGO_BIN_EXE_acus"), &copy).unwrap();
    let run = |args: &[&str]| {
        let mut command = std::process::Command::new(&copy);
        command
            .args(args)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("ACUS_CONFIG", "/nonexistent/acus.toml");
        decode(output_retrying_busy(&mut command).unwrap())
    };
    assert_eq!(run(&["self", "skill", "--install"]).0, 0);
    let skill = home.join(".claude/skills/acus/SKILL.md");
    assert!(skill.is_file());

    let (code, out, err) = run(&["self", "uninstall"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("removed"), "{out}");
    assert!(!skill.exists());
    assert!(!copy.exists());
    assert!(std::path::Path::new(env!("CARGO_BIN_EXE_acus")).is_file());
}

#[cfg(feature = "cmd-update")]
#[test]
fn uninstall_refuses_a_binary_not_named_acus() {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("renamed");
    std::fs::copy(env!("CARGO_BIN_EXE_acus"), &copy).unwrap();
    let mut command = std::process::Command::new(&copy);
    command
        .args(["self", "uninstall"])
        .env("HOME", dir.path())
        .env("USERPROFILE", dir.path());
    let out = output_retrying_busy(&mut command).unwrap();
    let (code, _, err) = decode(out);
    assert_eq!(code, 2);
    assert!(err.contains("refusing") && err.contains("hint:"), "{err}");
    assert!(copy.is_file());
}

#[test]
fn skill_install_writes_the_reference_topics() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path().to_str().unwrap();
    let (code, _, err) = acus_env(
        &["self", "skill", "--install"],
        &[("HOME", h), ("USERPROFILE", h)],
    );
    assert_eq!(code, 0, "{err}");
    for topic in ["find", "read", "patch", "git", "ctx"] {
        let file = home
            .path()
            .join(".claude/skills/acus/reference")
            .join(format!("{topic}.md"));
        assert!(file.is_file(), "{}", file.display());
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
fn skill_topic_prints_a_reference() {
    let (code, out, err) = acus(&["self", "skill", "--topic", "patch"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.lines().next(), Some("# patch"), "{out}");
    assert!(out.contains("*** Resolve Conflict"), "{out}");
}

#[test]
fn skill_topic_cannot_be_combined_with_full_or_install() {
    for args in [
        &["self", "skill", "--topic", "patch", "--full"][..],
        &["self", "skill", "--topic", "patch", "--install"][..],
    ] {
        let (code, _, err) = acus(args);
        assert_eq!(code, 2, "{args:?}: {err}");
    }
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
