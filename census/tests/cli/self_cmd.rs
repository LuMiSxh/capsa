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
