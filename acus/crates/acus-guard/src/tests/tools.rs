// SPDX-License-Identifier: MPL-2.0
//! Guard tests for the Read, Grep and Glob tools.

use super::*;

#[test]
fn turns_grep_tool_calls_into_acus_find() {
    let cwd = Some("/repo");
    let deny = |v| match grep_tool(&v, cwd) {
        Some(Verdict::Deny(Rule::GrepTool, r)) => r,
        other => panic!("{other:?}"),
    };
    let r = deny(
        json!({"pattern": "fn main", "path": "/repo/src", "output_mode": "content", "-C": 3, "type": "rust"}),
    );
    assert!(
        r.contains("`acus find 'fn main' 'src' -t 'rust' --block`"),
        "{r}"
    );
    let r = deny(json!({"pattern": "it's", "path": "/repo", "glob": "*.md"}));
    assert!(r.contains("`acus find \"it's\" -g '*.md' -l`"), "{r}");
    assert_eq!(
        grep_tool(&json!({"pattern": "x", "output_mode": "count"}), cwd),
        None
    );
    assert_eq!(
        grep_tool(&json!({"pattern": "a\nb", "multiline": true}), cwd),
        None
    );
}

#[test]
fn leaves_reads_outside_the_repository_alone() {
    let repo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".git")).unwrap();
    std::fs::create_dir_all(repo.path().join("crates/app")).unwrap();
    let cwd = repo.path().join("crates/app");
    let scope = Scope::new(cwd.to_str()).unwrap();
    let outside = other.path().join("agents/worker.md");
    let outside = outside.to_str().unwrap();
    let at = |cmd: &str| check_in(cmd, false, Some(&scope));
    for cmd in [
        format!("cat {outside}"),
        format!("head -50 {outside}"),
        format!("sed -n 1,20p {outside}"),
        format!("tail -n 20 {outside} ~/.config/app/main.py"),
        "cat ../../../elsewhere/a.rs".to_owned(),
        "cat /tmp/notes/a.md".to_owned(),
    ] {
        assert_eq!(at(&cmd), None, "{cmd}");
    }
    // Inside the repository (also from a subdirectory, `..` and absolute) nothing changes.
    let inside = repo.path().join("src/a.rs");
    for cmd in [
        "cat src/a.rs".to_owned(),
        "cat ../../README.md".to_owned(),
        format!("head -5 {}", inside.display()),
        format!("cat {outside} src/a.rs"),
    ] {
        assert!(
            matches!(at(&cmd), Some(Verdict::Deny(Rule::ReadShell, _))),
            "{cmd}"
        );
    }
    // Without a session directory the old behaviour holds.
    assert!(matches!(
        check(&format!("cat {outside}")),
        Some(Verdict::Deny(..))
    ));
    // Outside the repo, searches and build pipes are still refused.
    assert!(matches!(
        at("grep -rn foo /tmp/x"),
        Some(Verdict::Deny(Rule::Grep, _))
    ));
    assert!(matches!(
        at("cargo test | tail"),
        Some(Verdict::Deny(Rule::BuildPipe, _))
    ));
    if std::env::var_os("HOME").is_some() {
        assert!(!scope.inside("~/.claude/agents/worker.md"));
        assert!(!scope.inside("$HOME/.config/x.md"));
    }
    // PowerShell: .NET reads follow the same scope.
    let ps = |cmd: &str| check_ps(cmd, Some(&scope));
    assert_eq!(ps(&format!("[IO.File]::ReadAllText('{outside}')")), None);
    assert_eq!(ps(&format!("Get-Content {outside}")), None);
    assert!(ps("Get-Content src\\a.rs").is_some());
}

#[test]
fn read_tool_only_refuses_large_files_inside_the_repository() {
    let repo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".git")).unwrap();
    let big: String = (0..=READ_MAX)
        .map(|i| format!("fn f{i}() {{}}\n"))
        .collect();
    let inside = repo.path().join("a.rs");
    let outside = other.path().join("a.rs");
    std::fs::write(&inside, &big).unwrap();
    std::fs::write(&outside, &big).unwrap();
    let cwd = repo.path().to_str().unwrap();
    let scope = Scope::new(Some(cwd));
    let read = |p: &Path| {
        read_check(
            &json!({"file_path": p.to_str().unwrap()}),
            Some(cwd),
            scope.as_ref(),
        )
    };
    assert!(matches!(
        read(&inside),
        Some(Verdict::Deny(Rule::ReadTool, _))
    ));
    assert_eq!(read(&outside), None);
}

#[test]
fn refuses_whole_reads_of_large_sources_with_their_outline() {
    let small: String = (0..READ_MAX).map(|i| format!("fn f{i}() {{}}\n")).collect();
    assert_eq!(read_verdict("src/a.rs", Lang::Rust, &small), None);
    let large = format!("{small}fn last() {{}}\n");
    let Some(Verdict::Deny(Rule::ReadTool, reason)) = read_verdict("src/a.rs", Lang::Rust, &large)
    else {
        panic!("large file read whole");
    };
    assert!(reason.contains("src/a.rs has 301 lines"), "{reason}");
    assert!(
        reason.contains("… 241 more (acus outline src/a.rs)"),
        "{reason}"
    );
    assert!(reason.contains("fn f0 1-1"), "{reason}");
}
