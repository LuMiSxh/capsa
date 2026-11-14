// SPDX-License-Identifier: MPL-2.0

use crate::common::{acus, acus_env};

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
