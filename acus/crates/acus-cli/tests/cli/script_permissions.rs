// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::path::PathBuf;

/// A git repository with its own home and Claude config, so no real settings file is read.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("claude")).unwrap();
        std::fs::write(dir.path().join("repo/.env"), "SECRET=1\n").unwrap();
        std::fs::write(dir.path().join("repo/notes.txt"), "hello\n").unwrap();
        Sandbox { dir }
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn user(&self, json: &str) {
        std::fs::write(self.dir.path().join("claude/settings.json"), json).unwrap();
    }

    fn project(&self, json: &str) {
        std::fs::create_dir_all(self.repo().join(".claude")).unwrap();
        std::fs::write(self.repo().join(".claude/settings.json"), json).unwrap();
    }

    fn script(&self, code: &str) -> (i32, String, String) {
        self.invoke(&["script"], code)
    }

    fn batch(&self, input: &str) -> (i32, String, String) {
        self.invoke(&["batch"], input)
    }

    fn invoke(&self, args: &[&str], stdin: &str) -> (i32, String, String) {
        let path = |p: &str| self.dir.path().join(p).display().to_string();
        let env = [
            ("HOME", path("home")),
            ("USERPROFILE", path("home")),
            ("CLAUDE_CONFIG_DIR", path("claude")),
            ("ACUS_MANAGED_SETTINGS", path("managed.json")),
            ("XDG_STATE_HOME", path("state")),
            ("LOCALAPPDATA", path("state")),
        ];
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        acus_in_env(&self.repo(), args, stdin, &env)
    }
}

#[test]
fn a_denied_bash_rule_refuses_a_batch_run_line_and_runs_the_others() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"permissions": {"deny": ["Bash(rm:*)"]}}"#);
    let (code, out, _) = sandbox.batch("run 'rm notes.txt'\nfind hello\n");
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains(
            "error: the command `rm notes.txt` is denied by `Bash(rm:*)` in permissions.deny"
        ),
        "{out}"
    );
    assert!(out.contains("== find hello\n"), "{out}");
    assert!(sandbox.repo().join("notes.txt").exists());
}

#[test]
fn a_denied_bash_rule_refuses_run_and_names_the_rule() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"permissions": {"deny": ["Bash(rm:*)"]}}"#);
    let (code, _, err) = sandbox.script("return await run('rm notes.txt');");
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("is denied by `Bash(rm:*)` in permissions.deny"),
        "{err}"
    );
    assert!(sandbox.repo().join("notes.txt").exists());
    let (code, _, err) = sandbox.script("return await run('echo hi && rm notes.txt');");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("`Bash(rm:*)`"), "{err}");
    assert!(sandbox.repo().join("notes.txt").exists());
}

#[test]
fn an_ask_rule_refuses_run_with_a_hint() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"permissions": {"ask": ["Bash(echo:*)"]}}"#);
    let (code, _, err) = sandbox.script("return await run('echo hi');");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("needs approval by `Bash(echo:*)`"), "{err}");
    assert!(err.contains("its own tool call"), "{err}");
}

#[test]
fn a_patch_run_command_obeys_the_bash_rules() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"permissions": {"deny": ["Bash(echo:*)"]}}"#);
    let code = "return await patch(`*** Begin Patch\n*** Update File: notes.txt\n<<<<<<< SEARCH\nhello\n=======\nbye\n>>>>>>> REPLACE\n*** End Patch`, {run: 'echo hi'});";
    let (status, _, err) = sandbox.script(code);
    assert_eq!(status, 2, "{err}");
    assert!(err.contains("`Bash(echo:*)`"), "{err}");
}

#[test]
fn unrelated_commands_and_reads_pass() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"permissions": {"deny": ["Bash(rm:*)", "Read(./.env)"]}}"#);
    let (code, out, err) = sandbox.script("return (await run('echo hi')).text;");
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("hi"), "{out}");
    let (code, out, err) = sandbox.script("return await read('notes.txt');");
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("hello"), "{out}");
}

#[test]
fn a_read_deny_refuses_read_of_that_file() {
    let sandbox = Sandbox::new();
    sandbox.project(r#"{"permissions": {"deny": ["Read(./.env)"]}}"#);
    let (code, out, err) = sandbox.script("return await read('.env');");
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("is denied by `Read(./.env)` in permissions.deny"),
        "{err}"
    );
    assert!(!out.contains("SECRET"), "{out}");
}

#[test]
fn a_project_deny_beats_a_user_allow() {
    let sandbox = Sandbox::new();
    sandbox.user(r#"{"permissions": {"allow": ["Bash(rm:*)", "Read(./.env)"]}}"#);
    sandbox.project(r#"{"permissions": {"deny": ["Bash(rm:*)", "Read(.env)"]}}"#);
    let (code, _, err) = sandbox.script("return await run('rm notes.txt');");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("permissions.deny"), "{err}");
    let (code, _, err) = sandbox.script("return await read('.env');");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("`Read(.env)`"), "{err}");

    let user = Sandbox::new();
    user.user(r#"{"permissions": {"deny": ["Bash(echo:*)"]}}"#);
    let (code, _, err) = user.script("return await run('echo hi');");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("permissions.deny"), "{err}");
}
