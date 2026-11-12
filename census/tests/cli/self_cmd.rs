// SPDX-License-Identifier: MPL-2.0

use super::*;

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

    home.write(CONFIG_FILE, "mode = \"budget");
    let broken = census(home.path(), &cwd, &["self", "doctor", "--json"], "", &extra);
    assert_eq!(broken.status.code(), Some(1), "{}", text(&broken.stderr));
    let v: Value = serde_json::from_slice(&broken.stdout).expect("doctor --json is one value");
    assert_eq!(v["ok"], false);
    assert_eq!(v["groups"][1]["name"], "config");
    assert_eq!(v["groups"][1]["checks"][0]["status"], "fail");
}

#[test]
fn doctor_fix_repairs_the_status_line_and_hooks_then_has_nothing_to_fix() {
    let home = Scratch::new("cli-doctor-fix");
    let cwd = home.path().to_path_buf();
    let home_str = home.path().to_str().expect("utf-8 path");
    let extra = [("HOME", home_str)];
    let setup = census(home.path(), &cwd, &["self", "setup"], "", &extra);
    assert_eq!(setup.status.code(), Some(0), "{}", text(&setup.stderr));
    home.write("claude/settings.json", "{}\n");

    let fixed = census(
        home.path(),
        &cwd,
        &["self", "doctor", "--fix", "--json"],
        "",
        &extra,
    );
    assert_eq!(fixed.status.code(), Some(0), "{}", text(&fixed.stderr));
    let v: Value = serde_json::from_slice(&fixed.stdout).expect("doctor --json is one value");
    let names: Vec<&str> = v["fixed"]
        .as_array()
        .expect("fixed")
        .iter()
        .map(|c| c["name"].as_str().expect("check name"))
        .collect();
    assert_eq!(
        names,
        [
            "statusLine",
            "hook SessionStart",
            "hook UserPromptSubmit",
            "hook PostToolUse"
        ]
    );
    let settings = home.path().join("claude/settings.json");
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).expect("settings")).expect("json");
    assert_eq!(written["statusLine"]["command"], "census statusline --line");

    let again = census(home.path(), &cwd, &["self", "doctor", "--fix"], "", &extra);
    assert_eq!(again.status.code(), Some(0), "{}", text(&again.stderr));
    let out = text(&again.stdout);
    assert!(out.contains("nothing to fix"), "{out}");
}

#[test]
fn doctor_fix_leaves_a_missing_skill_to_its_hint() {
    let home = Scratch::new("cli-doctor-fix-missing-skill");
    let cwd = home.path().to_path_buf();
    let home_str = home.path().to_str().expect("utf-8 path");
    let extra = [("HOME", home_str)];
    std::fs::create_dir_all(home.path().join("claude")).unwrap();
    home.write("claude/settings.json", "{}\n");

    let fixed = census(
        home.path(),
        &cwd,
        &["self", "doctor", "--fix", "--json"],
        "",
        &extra,
    );
    assert_eq!(fixed.status.code(), Some(0), "{}", text(&fixed.stderr));
    let v: Value = serde_json::from_slice(&fixed.stdout).expect("doctor --json is one value");
    let names: Vec<&str> = v["fixed"]
        .as_array()
        .expect("fixed")
        .iter()
        .map(|c| c["name"].as_str().expect("check name"))
        .collect();
    assert!(!names.contains(&"claude skill"), "{names:?}");
    assert!(!home.path().join(".claude/skills/census/SKILL.md").exists());
    let skill = v["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|g| g["checks"].as_array().expect("checks"))
        .find(|c| c["name"] == "claude skill")
        .expect("the skill check");
    assert_eq!(skill["status"], "warn", "{skill}");
    assert_eq!(skill["hint"], "run census self setup", "{skill}");
}

#[test]
fn doctor_fix_keeps_a_status_line_census_does_not_own() {
    let home = Scratch::new("cli-doctor-fix-foreign");
    let cwd = home.path().to_path_buf();
    let home_str = home.path().to_str().expect("utf-8 path");
    let extra = [("HOME", home_str)];
    let setup = census(home.path(), &cwd, &["self", "setup"], "", &extra);
    assert_eq!(setup.status.code(), Some(0), "{}", text(&setup.stderr));
    home.write(
        "claude/settings.json",
        r#"{"statusLine":{"type":"command","command":"other-tool"}}"#,
    );

    let fixed = census(
        home.path(),
        &cwd,
        &["self", "doctor", "--fix", "--json"],
        "",
        &extra,
    );
    assert_eq!(fixed.status.code(), Some(0), "{}", text(&fixed.stderr));
    let v: Value = serde_json::from_slice(&fixed.stdout).expect("doctor --json is one value");
    let names: Vec<&str> = v["fixed"]
        .as_array()
        .expect("fixed")
        .iter()
        .map(|c| c["name"].as_str().expect("check name"))
        .collect();
    assert!(!names.contains(&"statusLine"), "{names:?}");
    assert_eq!(v["groups"][3]["checks"][0]["status"], "warn");
    let settings = home.path().join("claude/settings.json");
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).expect("settings")).expect("json");
    assert_eq!(written["statusLine"]["command"], "other-tool");
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
fn skill_target_selects_the_agent_dirs() {
    for (target, claude, codex) in [
        ("claude", true, false),
        ("codex", false, true),
        ("all", true, true),
    ] {
        let home = Scratch::new(&format!("cli-skill-{target}"));
        let cwd = home.path().to_path_buf();
        let out = census(
            home.path(),
            &cwd,
            &["self", "skill", "--install", "--target", target],
            "",
            &[],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{target}: {}",
            text(&out.stderr)
        );
        let written = |dir: &str| home.path().join(dir).join("SKILL.md").is_file();
        assert_eq!(written(".claude/skills/census"), claude, "{target}");
        assert_eq!(written(".agents/skills/census"), codex, "{target}");
    }
}

#[test]
fn doctor_warns_on_a_legacy_config_json_and_names_config_set() {
    let home = Scratch::new("cli-doctor-legacy");
    let cwd = home.path().to_path_buf();
    let setup = census(home.path(), &cwd, &["self", "setup"], "", &[]);
    assert_eq!(setup.status.code(), Some(0), "{}", text(&setup.stderr));
    home.write(&CONFIG_FILE.replace("config.toml", "config.json"), "{}");

    let out = census(home.path(), &cwd, &["self", "doctor", "--json"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("doctor --json is one value");
    let legacy = &v["groups"][1]["checks"][1];
    assert_eq!(legacy["name"], "legacy config");
    assert_eq!(legacy["status"], "warn");
    assert!(
        legacy["hint"]
            .as_str()
            .is_some_and(|h| h.contains("census self config set")),
        "{legacy}"
    );
}

#[test]
fn doctor_does_not_count_warnings_as_hook_errors() {
    let home = Scratch::new("cli-doctor-hook-warn");
    let cwd = home.path().to_path_buf();
    home.write(
        "state/census/hook.jsonl",
        &jsonl(&[
            json!({"ts": 10, "level": "error", "event": "hook.error", "message": "not JSON"}),
            json!({"ts": 30, "level": "warn", "event": "bulletin.refresh", "message": "full"}),
        ]),
    );
    let out = census(home.path(), &cwd, &["self", "doctor", "--json"], "", &[]);
    let v: Value = serde_json::from_slice(&out.stdout).expect("doctor --json is one value");
    let check = v["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|g| g["checks"].as_array().into_iter().flatten())
        .find(|c| c["name"] == "hook errors")
        .expect("the hook errors check");
    assert!(
        check.to_string().contains("1 error(s); last at 10"),
        "{check}"
    );
}
