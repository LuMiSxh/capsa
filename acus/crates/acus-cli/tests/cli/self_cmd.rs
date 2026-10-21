// SPDX-License-Identifier: MPL-2.0

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
    for name in ["update", "skill", "uninstall"] {
        assert!(out.contains(name), "{name}: {out}");
    }
}

#[test]
fn a_disabled_name_still_blocks_its_self_action() {
    let (code, _, err) = acus_env(&["self", "skill"], &[("ACUS_DISABLE", "skill")]);
    assert_eq!(code, 2);
    assert!(err.contains("disabled"), "{err}");
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
        decode(
            std::process::Command::new(&copy)
                .args(args)
                .env("HOME", &home)
                .env("USERPROFILE", &home)
                .env("ACUS_CONFIG", "/nonexistent/acus.toml")
                .output()
                .unwrap(),
        )
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
    let out = std::process::Command::new(&copy)
        .args(["self", "uninstall"])
        .env("HOME", dir.path())
        .env("USERPROFILE", dir.path())
        .output()
        .unwrap();
    let (code, _, err) = decode(out);
    assert_eq!(code, 2);
    assert!(err.contains("refusing") && err.contains("hint:"), "{err}");
    assert!(copy.is_file());
}
