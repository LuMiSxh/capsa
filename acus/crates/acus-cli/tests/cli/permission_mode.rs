// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use serde_json::{Value, json};
use std::path::PathBuf;

const SESSION: &str = "mode-test-session";

/// A git repository with its own home, Claude config and state directory.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("claude")).unwrap();
        std::fs::write(dir.path().join("repo/notes.txt"), "hello\n").unwrap();
        Sandbox { dir }
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn project(&self, json: &str) {
        std::fs::create_dir_all(self.repo().join(".claude")).unwrap();
        std::fs::write(self.repo().join(".claude/settings.json"), json).unwrap();
    }

    fn config(&self, toml: &str) {
        std::fs::write(self.dir.path().join("config.toml"), toml).unwrap();
    }

    fn state_file(&self) -> PathBuf {
        tool_state_dir(&self.dir.path().join("state"), "acus")
            .join("permission-mode")
            .join(format!("{SESSION}.json"))
    }

    fn recorded(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.state_file()).ok()?;
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(value["at"].is_u64(), "{text}");
        value["mode"].as_str().map(str::to_owned)
    }

    fn invoke(&self, args: &[&str], stdin: &str) -> (i32, String, String) {
        let path = |p: &str| self.dir.path().join(p).display().to_string();
        let env = [
            ("HOME", path("home")),
            ("USERPROFILE", path("home")),
            ("CLAUDE_CONFIG_DIR", path("claude")),
            ("ACUS_MANAGED_SETTINGS", path("managed.json")),
            ("ACUS_CONFIG", path("config.toml")),
            ("XDG_STATE_HOME", path("state")),
            ("LOCALAPPDATA", path("state")),
            ("CLAUDE_CODE_SESSION_ID", SESSION.to_owned()),
        ];
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        acus_in_env(&self.repo(), args, stdin, &env)
    }

    fn hook(&self, event: &Value) {
        let (code, out, err) = self.invoke(&["hook"], &event.to_string());
        assert_eq!(code, 0, "{out}{err}");
    }

    fn mode(&self, mode: &str) {
        self.hook(&json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": SESSION,
            "permission_mode": mode,
        }));
    }

    #[cfg(feature = "script")]
    fn script(&self, code: &str) -> (i32, String, String) {
        self.invoke(&["script"], code)
    }

    fn batch(&self, input: &str) -> (i32, String, String) {
        self.invoke(&["batch"], input)
    }
}

#[test]
fn user_prompt_submit_and_post_tool_use_record_the_permission_mode() {
    let sandbox = Sandbox::new();
    assert_eq!(sandbox.recorded(), None);
    sandbox.mode("plan");
    assert_eq!(sandbox.recorded().as_deref(), Some("plan"));
    sandbox.hook(&json!({
        "hook_event_name": "PostToolUse",
        "session_id": SESSION,
        "tool_name": "ExitPlanMode",
        "permission_mode": "acceptEdits",
    }));
    assert_eq!(sandbox.recorded().as_deref(), Some("acceptEdits"));
}

#[test]
fn pre_tool_use_records_the_mode_only_when_the_event_has_one() {
    let sandbox = Sandbox::new();
    let call = |extra: Value| {
        let mut event = json!({
            "hook_event_name": "PreToolUse",
            "session_id": SESSION,
            "tool_name": "Bash",
            "tool_input": {"command": "echo hi"},
        });
        event
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        sandbox.hook(&event);
    };
    call(json!({}));
    assert_eq!(sandbox.recorded(), None);
    call(json!({"permission_mode": "dontAsk"}));
    assert_eq!(sandbox.recorded().as_deref(), Some("dontAsk"));
}

#[test]
fn an_unknown_mode_or_a_missing_session_records_nothing() {
    let sandbox = Sandbox::new();
    sandbox.mode("plan");
    sandbox.mode("somethingNew");
    assert_eq!(sandbox.recorded().as_deref(), Some("plan"));
    sandbox.hook(&json!({"hook_event_name": "UserPromptSubmit", "permission_mode": "auto"}));
    assert_eq!(sandbox.recorded().as_deref(), Some("plan"));
}

#[test]
fn the_mode_hooks_print_nothing() {
    let sandbox = Sandbox::new();
    let event = json!({
        "hook_event_name": "PostToolUse",
        "session_id": SESSION,
        "permission_mode": "plan",
    });
    let (code, out, err) = sandbox.invoke(&["hook"], &event.to_string());
    assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));
}

#[cfg(feature = "script")]
#[test]
fn plan_mode_refuses_a_script_write_and_run_but_not_a_read() {
    let sandbox = Sandbox::new();
    sandbox.mode("plan");
    let (code, _, err) = sandbox.script("return await write('new.txt', 'x');");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("refused in plan mode"), "{err}");
    assert!(!sandbox.repo().join("new.txt").exists());
    let (code, _, err) = sandbox.script("return await run('echo hi');");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("refused in plan mode"), "{err}");
    let (code, out, err) = sandbox.script("return await read('notes.txt');");
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("hello"), "{out}");
}

#[test]
fn plan_mode_refuses_a_batch_run_line_and_runs_the_others() {
    let sandbox = Sandbox::new();
    sandbox.mode("plan");
    let (code, out, _) = sandbox.batch("run 'echo hi'\nfind hello\n");
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("refused in plan mode"), "{out}");
    assert!(out.contains("== find hello\n"), "{out}");
}

#[cfg(feature = "script")]
#[test]
fn dont_ask_refuses_a_run_without_an_allow_rule_and_lets_a_listed_one_go() {
    let sandbox = Sandbox::new();
    sandbox.mode("dontAsk");
    let (code, _, err) = sandbox.script("return await run('echo hi');");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("refused in dontAsk mode"), "{err}");
    sandbox.project(r#"{"permissions": {"allow": ["Bash(echo:*)"]}}"#);
    let (code, out, err) = sandbox.script("return (await run('echo hi')).text;");
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("hi"), "{out}");
}

#[cfg(feature = "script")]
#[test]
fn without_a_recorded_mode_a_script_runs_as_before() {
    let sandbox = Sandbox::new();
    let (code, out, err) = sandbox.script("return (await run('echo hi')).text;");
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("hi"), "{out}");
    sandbox.config("[script]\nstrict = false\n");
    let (code, _, err) = sandbox.script("await write('new.txt', 'x'); return 1;");
    assert_eq!(code, 0, "{err}");
}

#[cfg(feature = "script")]
#[test]
fn a_script_edit_of_a_protected_path_needs_an_allow_rule_outside_bypass() {
    let sandbox = Sandbox::new();
    sandbox.config("[script]\nstrict = true\n");
    sandbox.mode("acceptEdits");
    let (code, _, err) = sandbox.script("await write('ok.txt', 'x'); return 1;");
    assert_eq!(code, 0, "{err}");
    for path in [".git/hooks/pre-commit", ".claude/settings.local.json"] {
        let (code, _, err) = sandbox.script(&format!("return await write('{path}', 'x');"));
        assert_eq!(code, 2, "{path}: {err}");
        assert!(
            err.contains("protected path; run the edit as its own tool call"),
            "{err}"
        );
        assert!(!sandbox.repo().join(path).exists(), "{path}");
    }
    sandbox.mode("bypassPermissions");
    let (code, _, err) = sandbox.script("await write('.git/hooks/pre-commit', 'x'); return 1;");
    assert_eq!(code, 0, "{err}");
}

#[cfg(feature = "script")]
#[test]
fn strict_refuses_an_unlisted_run_in_default_mode_with_a_hint() {
    let sandbox = Sandbox::new();
    sandbox.config("[script]\nstrict = true\n");
    sandbox.mode("default");
    let (code, _, err) = sandbox.script("return await run('echo hi');");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("refused in default mode"), "{err}");
    assert!(
        err.contains("no allow rule; add one or run it as its own tool call"),
        "{err}"
    );
    sandbox.mode("bypassPermissions");
    let (code, _, err) = sandbox.script("return (await run('echo hi')).text;");
    assert_eq!(code, 0, "{err}");
}
