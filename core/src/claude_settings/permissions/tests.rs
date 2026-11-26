// SPDX-License-Identifier: MPL-2.0
use super::path::{self, Place};
use super::{Permissions, bash, fetch};
use std::path::{Path, PathBuf};

#[test]
fn bash_rules_match_the_commands_they_name() {
    let cases = [
        ("rm:*", "rm x", true),
        ("rm:*", "rm", true),
        ("rm:*", "rmdir x", false),
        ("npm test:*", "npm test --watch", true),
        ("npm test:*", "npm run test", false),
        ("git status", "git status", true),
        ("git status", "git status -s", false),
        ("git *", "git push", true),
        ("git *", "gitk", false),
        ("git*", "gitk", true),
        ("ls *", "ls", true),
        ("* --version", "node --version", true),
        ("git * main", "git push origin main", true),
        ("git * main", "git push origin dev", false),
        ("rm:*", "cd build && rm -rf out", true),
        ("rm:*", "echo done; rm x", true),
        ("rm:*", "cat f | rm x", true),
        ("rm:*", "true || rm x", true),
        ("rm:*", "echo $(rm x)", true),
        ("rm:*", "echo `rm x`", true),
        ("rm:*", "FOO=1 rm x", true),
        ("rm:*", "timeout 5 rm x", true),
        ("rm:*", "nice -n 5 rm x", true),
        ("rm:*", "time rm x", true),
        ("rm:*", "echo 'a && rm x'", false),
        ("rm:*", "echo \"a; rm x\"", false),
        ("rm:*", "ls 2>&1", false),
        ("npm test", "cd x && npm test", true),
        ("rm:*", "if true; then rm -rf build; fi", true),
        ("rm:*", "for f in a b; do rm $f; done", true),
        ("rm:*", "while true; do rm x; done", true),
        ("rm:*", "until false; do rm x; done", true),
        ("rm:*", "if a; then b; elif c; then d; else rm x; fi", true),
        ("rm:*", "{ rm x; }", true),
        ("rm:*", "! rm x", true),
        ("rm:*", "( rm x )", true),
        ("rm:*", "bash -c 'rm -rf build'", true),
        ("rm:*", "sh -c \"cd x && rm y\"", true),
        ("rm:*", "/bin/bash -lc 'rm x'", true),
        ("rm:*", "eval rm x", true),
        ("rm:*", "eval 'rm x'", true),
        ("rm:*", "bash -c \"sh -c 'rm x'\"", true),
        ("rm:*", "bash -c 'ls'", false),
        ("rm:*", "bash script.sh", false),
        ("rm:*", "echo then rm", false),
        ("rm:*", "echo 'bash -c rm'", false),
        ("rm:*", "bash -o pipefail -c 'rm -rf x'", true),
        ("rm:*", "bash -O extglob -c 'rm -rf x'", true),
        ("rm:*", "bash +o history -c 'rm x'", true),
        ("rm:*", "bash +O extglob -c 'rm x'", true),
        ("rm:*", "sh -ec 'rm x'", true),
        ("rm:*", "zsh -xc 'rm x'", true),
        ("rm:*", "dash -lc 'rm x'", true),
        ("rm:*", "bash --norc -c 'rm x'", true),
        ("rm:*", "bash --rcfile my.rc -c 'rm x'", true),
        ("rm:*", "bash --init-file my.rc -ic 'rm x'", true),
        ("rm:*", "bash -c -o pipefail 'rm x'", true),
        ("rm:*", "bash -eo pipefail -c 'rm x'", true),
        ("rm:*", "bash -c -- 'rm x'", true),
        ("rm:*", "bash -o pipefail -c 'ls'", false),
        ("rm:*", "bash -o pipefail script.sh", false),
        ("rm:*", "bash -o pipefail script.sh -c rm", false),
        ("rm:*", "bash -- -c rm", false),
    ];
    for (spec, command, expected) in cases {
        assert_eq!(bash::matches(spec, command), expected, "{spec} / {command}");
    }
}

#[test]
fn an_unparseable_shell_line_checks_every_word_after_c() {
    for command in [
        "bash --future-flag x -c 'rm -rf x'",
        "bash --future-flag -c ls 'rm x'",
    ] {
        assert!(bash::matches("rm:*", command), "{command}");
    }
    assert!(!bash::matches("rm:*", "bash --future-flag script.sh"));
    assert!(!bash::covered(
        &[Some("bash:*")],
        "bash -o pipefail -c 'rm x'"
    ));
}

#[test]
fn domain_rules_match_host_and_subdomains_but_not_lookalikes() {
    let host = |u: &str| fetch::host(u).unwrap();
    assert_eq!(host("https://Example.com:8080/a?b#c"), "example.com");
    assert_eq!(host("https://good.com@evil.com/x"), "evil.com");
    assert_eq!(host("http://[::1]:80/"), "::1");
    assert!(fetch::host("https:///x").is_none());
    assert!(fetch::matches("domain:example.com", "example.com"));
    assert!(fetch::matches("domain:example.com", "docs.example.com"));
    assert!(!fetch::matches("domain:example.com", "notexample.com"));
    assert!(!fetch::matches("domain:example.com", "example.com.evil.io"));
    assert!(fetch::matches("domain:*.example.com", "a.example.com"));
    assert!(!fetch::matches("domain:*.example.com", "example.com"));
    assert!(!fetch::matches("example.com", "example.com"));
}

struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("repo/src")).unwrap();
        std::fs::create_dir_all(dir.path().join("home")).unwrap();
        Project { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("repo").canonicalize().unwrap()
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let file = self.dir.path().join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        file
    }

    fn permissions(&self, files: Vec<PathBuf>) -> Permissions {
        let home = self.dir.path().join("home").canonicalize().unwrap();
        Permissions::with_files(files, self.root(), self.root(), Some(home))
    }

    /// The settings are in one project file.
    fn with(&self, json: &str) -> Permissions {
        let file = self.write("repo/.claude/settings.json", json);
        self.permissions(vec![file])
    }
}

fn denies(result: Result<(), super::Refusal>, rule: &str) -> bool {
    result.is_err_and(|e| e.message.contains(&format!("`{rule}`")))
}

#[test]
fn deny_and_ask_refuse_and_name_the_rule() {
    let project = Project::new();
    let p = project.with(r#"{"permissions": {"deny": ["Bash(rm:*)"]}}"#);
    let err = p.check_bash("rm x").unwrap_err();
    assert!(
        err.message
            .contains("is denied by `Bash(rm:*)` in permissions.deny")
    );
    assert!(p.check_bash("echo hi").is_ok());

    let project = Project::new();
    let p = project.with(r#"{"permissions": {"ask": ["Bash(git push:*)"]}}"#);
    let err = p.check_bash("git push origin").unwrap_err();
    assert!(err.message.contains("needs approval by `Bash(git push:*)`"));
    assert!(err.hint.as_deref().unwrap().contains("its own tool call"));
}

#[test]
fn deny_wins_over_ask_and_a_project_deny_wins_over_a_user_allow() {
    let project = Project::new();
    let user = project.write(
        "claude/settings.json",
        r#"{"permissions": {"allow": ["Bash(rm:*)"], "ask": ["Bash(rm:*)"]}}"#,
    );
    let shared = project.write(
        "repo/.claude/settings.json",
        r#"{"permissions": {"deny": ["Bash(rm:*)"]}}"#,
    );
    let p = project.permissions(vec![user.clone(), shared]);
    let err = p.check_bash("rm x").unwrap_err();
    assert!(err.message.contains("permissions.deny"), "{}", err.message);
    let p = project.permissions(vec![user]);
    assert!(
        p.check_bash("rm x")
            .unwrap_err()
            .message
            .contains("permissions.ask")
    );
}

#[test]
fn a_whole_tool_rule_covers_every_call() {
    let project = Project::new();
    let p = project.with(r#"{"permissions": {"deny": ["Bash", "WebFetch"]}}"#);
    assert!(p.check_bash("anything").is_err());
    assert!(p.check_fetch("https://example.com").is_err());
    assert!(p.check_read(Path::new("src/a.rs")).is_ok());
}

#[test]
fn an_unreadable_settings_file_refuses_every_call() {
    let project = Project::new();
    let p = project.with("{not json");
    let err = p.check_bash("echo hi").unwrap_err();
    assert!(err.message.contains("cannot read"), "{}", err.message);
}

#[test]
fn read_edit_and_write_rules_follow_the_path_prefixes() {
    let project = Project::new();
    let p = project.with(
        r#"{"permissions": {"deny": [
            "Read(./.env)", "Read(/secrets/**)", "Read(*.pem)", "Read(~/.ssh/**)", "Edit(src/gen/)"
        ]}}"#,
    );
    let root = project.root();
    let home = project.dir.path().join("home");
    let read = |path: &Path| p.check_read(path);
    assert!(denies(read(Path::new(".env")), "Read(./.env)"));
    assert!(denies(read(&root.join(".env")), "Read(./.env)"));
    assert!(read(Path::new("src/.env")).is_ok());
    assert!(denies(
        read(Path::new("secrets/a/b.txt")),
        "Read(/secrets/**)"
    ));
    assert!(denies(read(Path::new("src/deep/key.pem")), "Read(*.pem)"));
    assert!(denies(read(&home.join(".ssh/id_rsa")), "Read(~/.ssh/**)"));
    assert!(read(&home.join(".config/x")).is_ok());
    assert!(read(Path::new("src/main.rs")).is_ok());
    assert!(read(Path::new("src/../src/main.rs")).is_ok());
    assert!(denies(read(Path::new("src/../.env")), "Read(./.env)"));
    assert!(p.check_edit(Path::new("src/gen/out.rs")).is_err());
    assert!(p.check_edit(Path::new("src/main.rs")).is_ok());

    let project = Project::new();
    let p = project.with(r#"{"permissions": {"ask": ["Write(docs/**)"], "deny": ["Edit(lock)"]}}"#);
    assert!(p.check_edit(Path::new("docs/a.md")).is_err());
    assert!(p.check_edit(Path::new("lock")).is_err());
    assert!(p.check_read(Path::new("docs/a.md")).is_ok());
}

#[test]
fn absolute_rules_use_a_double_slash() {
    let project = Project::new();
    let target = project.write("outside/data.txt", "x");
    let mut spec = path::targets(&target, project.dir.path()).pop().unwrap();
    spec.insert(0, '/');
    let p = project.with(&format!(
        r#"{{"permissions": {{"deny": ["Read({spec})"]}}}}"#
    ));
    assert!(p.check_read(&target).is_err());
    assert!(
        p.check_read(&project.dir.path().join("outside/other.txt"))
            .is_ok()
    );
}

#[test]
fn place_resolves_the_prefixes_it_is_given() {
    let project = Project::new();
    let root = project.root();
    let place = Place::new(&root, &root, None);
    let target = path::targets(Path::new("a/b"), &root).remove(0);
    assert!(path::matches("a/**", &target, &place));
    assert!(!path::matches("~/a/**", &target, &place));
    assert!(!path::matches("c/**", &target, &place));
}
