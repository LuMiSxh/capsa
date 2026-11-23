// SPDX-License-Identifier: MPL-2.0

use super::Skill;
use super::{MANIFEST_FILE, SkillState, Target, install, installed_target, state, uninstall};

const SKILL: Skill = Skill {
    tool: "demo",
    text: "# demo\n",
    extra: &[],
};

const EXTRA: Skill = Skill {
    tool: "demo",
    text: "# demo\n",
    extra: &[
        ("reference/one.md", "# one\n"),
        ("reference/two.md", "# two\n"),
    ],
};

const SHRUNK: Skill = Skill {
    tool: "demo",
    text: "# demo\n",
    extra: &[("reference/one.md", "# one\n")],
};

fn skill_file(home: &std::path::Path, dir: &str) -> std::path::PathBuf {
    home.join(dir).join("demo").join("SKILL.md")
}

const CLAUDE: &str = ".claude/skills";
const CODEX: &str = ".agents/skills";

#[test]
fn target_dirs_follow_the_agent_layout() {
    let home = std::path::Path::new("/h");
    assert_eq!(
        Target::Claude.dirs(home, "demo"),
        [home.join(".claude/skills/demo")]
    );
    assert_eq!(
        Target::Codex.dirs(home, "demo"),
        [home.join(".agents/skills/demo")]
    );
    assert_eq!(Target::All.dirs(home, "demo").len(), 2);
}

#[test]
fn install_writes_only_the_chosen_targets_dirs() {
    for target in [Target::Claude, Target::Codex, Target::All] {
        let t = tempfile::tempdir().unwrap();
        let written = install(t.path(), &SKILL, target).unwrap();
        assert_eq!(
            written.len(),
            target.dirs(t.path(), "demo").len(),
            "{target:?}"
        );
        let has = |dir: &str| skill_file(t.path(), dir).is_file();
        assert_eq!(has(CLAUDE), target != Target::Codex, "{target:?}");
        assert_eq!(has(CODEX), target != Target::Claude, "{target:?}");
        assert_eq!(std::fs::read_to_string(&written[0]).unwrap(), SKILL.text);
        assert!(!t.path().join(".claude/settings.json").exists());
    }
}

#[test]
fn uninstall_removes_only_the_skill_files() {
    let t = tempfile::tempdir().unwrap();
    install(t.path(), &SKILL, Target::All).unwrap();
    let other = t.path().join(CODEX).join("demo").join("notes.md");
    std::fs::write(&other, "mine").unwrap();

    let planned = uninstall(t.path(), &SKILL, true).unwrap();
    assert_eq!(planned.len(), 2);
    assert!(skill_file(t.path(), CLAUDE).is_file());

    let removed = uninstall(t.path(), &SKILL, false).unwrap();
    assert_eq!(removed.len(), 2);
    assert!(!t.path().join(CLAUDE).join("demo").exists());
    assert!(other.is_file(), "a file the user wrote stays");
    assert!(!skill_file(t.path(), CODEX).exists());
    assert!(uninstall(t.path(), &SKILL, false).unwrap().is_empty());
}

#[test]
fn uninstall_leaves_a_non_empty_parent_directory() {
    let t = tempfile::tempdir().unwrap();
    install(t.path(), &SKILL, Target::Codex).unwrap();
    let dir = t.path().join(CODEX).join("demo");
    std::fs::write(dir.join("extra.md"), "x").unwrap();
    uninstall(t.path(), &SKILL, false).unwrap();
    assert!(dir.is_dir());
    assert!(dir.join("extra.md").is_file());
}

#[test]
fn state_tracks_missing_outdated_and_current() {
    let t = tempfile::tempdir().unwrap();
    assert_eq!(state(t.path(), &SKILL, Target::All), SkillState::Missing);
    install(t.path(), &SKILL, Target::Claude).unwrap();
    assert_eq!(state(t.path(), &SKILL, Target::Claude), SkillState::Current);
    assert_eq!(state(t.path(), &SKILL, Target::All), SkillState::Current);
    std::fs::write(skill_file(t.path(), CLAUDE), "# old\n").unwrap();
    assert_eq!(
        state(t.path(), &SKILL, Target::Claude),
        SkillState::Outdated
    );
    assert_eq!(state(t.path(), &SKILL, Target::Codex), SkillState::Missing);
}

#[test]
fn install_writes_the_extra_files_beside_the_skill() {
    let t = tempfile::tempdir().unwrap();
    let written = install(t.path(), &EXTRA, Target::Claude).unwrap();
    assert_eq!(written.len(), 3);
    let dir = t.path().join(CLAUDE).join("demo");
    assert_eq!(
        std::fs::read_to_string(dir.join("reference/one.md")).unwrap(),
        "# one\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("reference/two.md")).unwrap(),
        "# two\n"
    );
}

#[test]
fn uninstall_removes_the_extra_files_and_their_empty_directory() {
    let t = tempfile::tempdir().unwrap();
    install(t.path(), &EXTRA, Target::Codex).unwrap();
    let dir = t.path().join(CODEX).join("demo");
    let removed = uninstall(t.path(), &EXTRA, false).unwrap();
    assert_eq!(removed.len(), 3);
    assert!(!dir.exists(), "the skill directory and reference/ are gone");
    assert!(uninstall(t.path(), &EXTRA, false).unwrap().is_empty());
}

#[test]
fn state_is_outdated_when_an_extra_file_is_missing_or_differs() {
    let t = tempfile::tempdir().unwrap();
    install(t.path(), &EXTRA, Target::Claude).unwrap();
    assert_eq!(state(t.path(), &EXTRA, Target::Claude), SkillState::Current);
    let one = t.path().join(CLAUDE).join("demo/reference/one.md");
    std::fs::remove_file(&one).unwrap();
    assert_eq!(
        state(t.path(), &EXTRA, Target::Claude),
        SkillState::Outdated
    );
    std::fs::write(&one, "# changed\n").unwrap();
    assert_eq!(
        state(t.path(), &EXTRA, Target::Claude),
        SkillState::Outdated
    );
}

#[test]
fn installed_target_reports_where_the_skill_lives() {
    let t = tempfile::tempdir().unwrap();
    let h = t.path();
    assert_eq!(installed_target(h, "demo"), None);
    std::fs::create_dir_all(h.join(CODEX).join("demo")).unwrap();
    assert_eq!(installed_target(h, "demo"), Some(Target::Codex));
    std::fs::create_dir_all(h.join(CLAUDE).join("demo")).unwrap();
    assert_eq!(installed_target(h, "demo"), Some(Target::All));
    assert_eq!(installed_target(h, "other"), None);
}

#[test]
fn install_writes_a_manifest_of_the_files_it_wrote() {
    let t = tempfile::tempdir().unwrap();
    install(t.path(), &EXTRA, Target::Claude).unwrap();
    let manifest = t.path().join(CLAUDE).join("demo").join(MANIFEST_FILE);
    assert_eq!(
        std::fs::read_to_string(manifest).unwrap(),
        "SKILL.md\nreference/one.md\nreference/two.md\n"
    );
}

#[test]
fn install_removes_a_file_the_manifest_lists_but_the_new_skill_drops() {
    let t = tempfile::tempdir().unwrap();
    install(t.path(), &EXTRA, Target::Claude).unwrap();
    install(t.path(), &SHRUNK, Target::Claude).unwrap();
    let dir = t.path().join(CLAUDE).join("demo");
    assert!(!dir.join("reference/two.md").exists());
    assert!(dir.join("reference/one.md").is_file());
    assert_eq!(
        state(t.path(), &SHRUNK, Target::Claude),
        SkillState::Current
    );
}

#[test]
fn install_removes_reference_files_of_an_install_without_a_manifest() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join(CLAUDE).join("demo");
    std::fs::create_dir_all(dir.join("reference")).unwrap();
    std::fs::write(dir.join("SKILL.md"), "# old\n").unwrap();
    std::fs::write(dir.join("reference/ctx.md"), "# ctx\n").unwrap();
    install(t.path(), &EXTRA, Target::Claude).unwrap();
    assert!(!dir.join("reference/ctx.md").exists());
    assert!(dir.join("reference/one.md").is_file());
}

#[test]
fn install_keeps_a_user_file_beside_the_skill() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join(CLAUDE).join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.md"), "mine").unwrap();
    install(t.path(), &EXTRA, Target::Claude).unwrap();
    install(t.path(), &SHRUNK, Target::Claude).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.md")).unwrap(),
        "mine"
    );
}

#[test]
fn a_manifest_entry_outside_the_skill_directory_is_not_removed() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join(CLAUDE).join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(t.path().join("outside.md"), "mine").unwrap();
    std::fs::write(dir.join(MANIFEST_FILE), "../../../outside.md\n").unwrap();
    install(t.path(), &SKILL, Target::Claude).unwrap();
    assert!(t.path().join("outside.md").is_file());
}

#[test]
fn uninstall_removes_the_manifest_and_the_files_it_lists() {
    let t = tempfile::tempdir().unwrap();
    install(t.path(), &EXTRA, Target::Claude).unwrap();
    let dir = t.path().join(CLAUDE).join("demo");
    let manifest = dir.join(MANIFEST_FILE);
    let listed = std::fs::read_to_string(&manifest).unwrap() + "reference/ctx.md\n";
    std::fs::write(&manifest, listed).unwrap();
    std::fs::write(dir.join("reference/ctx.md"), "# ctx\n").unwrap();
    uninstall(t.path(), &EXTRA, false).unwrap();
    assert!(!manifest.exists());
    assert!(!dir.join("reference/ctx.md").exists());
    assert!(!dir.exists());
}

#[test]
fn state_is_outdated_while_a_listed_file_is_no_longer_shipped() {
    let t = tempfile::tempdir().unwrap();
    install(t.path(), &EXTRA, Target::Claude).unwrap();
    assert_eq!(
        state(t.path(), &SHRUNK, Target::Claude),
        SkillState::Outdated
    );
    install(t.path(), &SHRUNK, Target::Claude).unwrap();
    assert_eq!(
        state(t.path(), &SHRUNK, Target::Claude),
        SkillState::Current
    );
}
