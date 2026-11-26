// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use serde_json::{Value, json};

use crate::hint::Hint;

use super::{
    Removal, Step, command_line, env_value, event_entries, is_owned, items, matcher_of, read,
    remove_env, remove_hook, runs, set_env, set_hook, write,
};

fn has_hook<'a>(v: &'a Value, event: &str, owns: impl Fn(&str) -> bool) -> Option<&'a str> {
    event_entries(v, event)
        .iter()
        .find(|g| items(g).any(|i| is_owned(i, &owns)))
        .map(|g| matcher_of(g).unwrap_or(""))
}

const COMMAND: &str = "demo-hook run";

fn owned(command: &str) -> bool {
    runs(command, "demo-hook", "run")
}

fn written(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn adds_the_hook_to_a_missing_file() {
    let t = tempfile::tempdir().unwrap();
    let mut s = read(&t.path().join("settings.json")).unwrap();
    let step = set_hook(
        &mut s.value,
        "SessionStart",
        Some("startup"),
        COMMAND,
        owned,
    );
    assert_eq!(step, Step::Install);
    write(&s).unwrap();
    assert_eq!(
        written(&s.path)["hooks"]["SessionStart"],
        json!([{"matcher": "startup", "hooks": [{"type": "command", "command": COMMAND}]}])
    );
    assert!(!t.path().join("settings.json.bak").exists());
}

#[test]
fn set_hook_installs_replaces_or_keeps_the_hook() {
    let startup = json!({"hooks": {"SessionStart": [
        {"matcher": "startup", "hooks": [{"type": "command", "command": COMMAND}]}
    ]}});
    let cases = [
        (
            json!({}),
            "PreToolUse",
            None,
            Step::Install,
            json!({"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": COMMAND}]}]}}),
            "",
        ),
        (
            startup.clone(),
            "SessionStart",
            Some("startup|clear"),
            Step::Replace,
            json!({"hooks": {"SessionStart": [
                {"matcher": "startup|clear", "hooks": [{"type": "command", "command": COMMAND}]}
            ]}}),
            "startup|clear",
        ),
        (
            startup.clone(),
            "SessionStart",
            Some("startup"),
            Step::Set,
            startup,
            "startup",
        ),
    ];
    for (start, event, matcher, expected_step, expected, expected_matcher) in cases {
        let mut v = start;
        let step = set_hook(&mut v, event, matcher, COMMAND, owned);
        assert_eq!(step, expected_step, "{event} {matcher:?}");
        assert_eq!(
            step.writes(),
            expected_step != Step::Set,
            "{event} {matcher:?}"
        );
        assert_eq!(v, expected, "{event} {matcher:?}");
        assert_eq!(
            has_hook(&v, event, owned),
            Some(expected_matcher),
            "{event} {matcher:?}"
        );
    }
}

#[test]
fn set_hook_keeps_foreign_hooks_and_entries_in_order() {
    let mut v = json!({"model": "x", "hooks": {"PreToolUse": [
        {"matcher": "Edit", "hooks": [{"type": "command", "command": "fmt.sh"}]}
    ], "Stop": [{"hooks": []}]}, "theme": "dark"});
    set_hook(&mut v, "PreToolUse", Some("Bash"), COMMAND, owned);
    assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "fmt.sh");
    assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    assert_eq!(v["hooks"]["Stop"], json!([{"hooks": []}]));
    let keys: Vec<_> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["model", "hooks", "theme"]);

    let mut v = json!({"hooks": {"PreToolUse": [
        {"matcher": "Bash", "hooks": [
            {"type": "command", "command": "demo-hook run --old"},
            {"type": "command", "command": "keep.sh"}
        ]}
    ]}});
    assert_eq!(
        set_hook(&mut v, "PreToolUse", Some("Bash"), COMMAND, owned),
        Step::Replace
    );
    let groups = v["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(
        groups[0]["hooks"],
        json!([{"type": "command", "command": "keep.sh"}])
    );
    assert_eq!(groups[1]["matcher"], "Bash");
    assert_eq!(
        set_hook(&mut v, "PreToolUse", Some("Bash"), COMMAND, owned),
        Step::Set
    );
}

#[test]
fn a_write_keeps_a_backup_or_refuses_a_changed_file() {
    let setters: [fn(&mut Value); 2] = [
        |v| {
            set_hook(v, "PreToolUse", None, COMMAND, owned);
        },
        |v| {
            set_env(v, "ANTHROPIC_BASE_URL", "http://x");
        },
    ];
    for set in setters {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("settings.json");
        std::fs::write(&path, r#"{"theme":"light"}"#).unwrap();
        let mut s = read(&path).unwrap();
        set(&mut s.value);
        std::fs::write(&path, r#"{"theme":"dark"}"#).unwrap();

        let err = write(&s).unwrap_err();
        assert!(err.to_string().contains("changed while writing"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"theme":"dark"}"#
        );
        let leftover = std::fs::read_dir(t.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().contains("capsa-tmp"));
        assert!(!leftover);
    }

    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("settings.json");
    let old = "{\n  \"theme\": \"light\"\n}\n";
    std::fs::write(&path, old).unwrap();
    let mut s = read(&path).unwrap();
    set_hook(&mut s.value, "PreToolUse", None, COMMAND, owned);
    write(&s).unwrap();
    assert_eq!(
        std::fs::read_to_string(t.path().join("settings.json.bak")).unwrap(),
        old
    );
    assert_eq!(written(&path)["theme"], "light");
    assert_eq!(
        written(&path)["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        COMMAND
    );
}

#[test]
fn read_refuses_wrong_shapes_for_the_document_hooks_and_env() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("settings.json");
    std::fs::write(&path, "[1]").unwrap();
    assert!(
        read(&path)
            .unwrap_err()
            .to_string()
            .contains("not a JSON object")
    );
    std::fs::write(&path, r#"{"hooks": []}"#).unwrap();
    assert!(read(&path).unwrap_err().to_string().contains("hooks"));
    std::fs::write(&path, r#"{"hooks": {"Stop": {}}}"#).unwrap();
    assert!(read(&path).unwrap_err().to_string().contains("hooks.Stop"));

    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("settings.json");
    std::fs::write(&path, r#"{"env": ["ANTHROPIC_BASE_URL"]}"#).unwrap();
    let err = read(&path).unwrap_err();
    assert!(err.to_string().contains("env in"), "{err}");
    assert!(err.hint().is_some());
}

#[cfg(unix)]
#[test]
fn a_symlinked_settings_file_stays_a_link() {
    let t = tempfile::tempdir().unwrap();
    let target = t.path().join("real.json");
    std::fs::write(&target, "{}").unwrap();
    let link = t.path().join("settings.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut s = read(&link).unwrap();
    assert_eq!(s.path, std::fs::canonicalize(&target).unwrap());
    set_hook(&mut s.value, "PreToolUse", None, COMMAND, owned);
    write(&s).unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        written(&target)["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        COMMAND
    );
}

#[test]
fn remove_hook_keeps_foreign_hooks_and_drops_an_empty_event() {
    let t = tempfile::tempdir().unwrap();
    let mut s = read(&t.path().join("settings.json")).unwrap();
    set_hook(&mut s.value, "SessionStart", None, "other-tool run", |_| {
        false
    });
    set_hook(&mut s.value, "SessionStart", None, COMMAND, owned);
    assert!(remove_hook(&mut s.value, "SessionStart", owned));
    assert_eq!(
        s.value,
        json!({"hooks": {"SessionStart": [
            {"hooks": [{"type": "command", "command": "other-tool run"}]}
        ]}})
    );
    assert!(!remove_hook(&mut s.value, "SessionStart", owned));

    let t = tempfile::tempdir().unwrap();
    let mut s = read(&t.path().join("settings.json")).unwrap();
    set_hook(&mut s.value, "SessionStart", None, COMMAND, owned);
    assert!(remove_hook(&mut s.value, "SessionStart", owned));
    assert_eq!(s.value, json!({}));
}

#[test]
fn runs_recognises_the_plain_quoted_and_exe_forms() {
    assert!(runs(r#""/My Apps/demo" hook"#, "demo", "hook"));
    assert!(runs(
        r#""C:\Program Files\demo.exe" statusline"#,
        "demo",
        "statusline"
    ));
    assert!(runs(
        r#"demo hook 2>/dev/null || "$HOME/.cargo/bin/demo" hook || true"#,
        "demo",
        "hook"
    ));
    assert!(!runs("my-line", "demo", "statusline"));
    assert!(!runs("demo statusline --line", "demo", "hook"));
    assert!(!runs("pdemo hook", "demo", "hook"));

    #[cfg(not(windows))]
    {
        assert_eq!(
            command_line(Path::new("/usr/bin/demo"), "hook"),
            "/usr/bin/demo hook"
        );
        assert_eq!(
            command_line(Path::new("/My Apps/demo"), "hook"),
            "\"/My Apps/demo\" hook"
        );
    }
}

#[cfg(windows)]
#[test]
fn a_windows_binary_path_is_quoted_with_forward_slashes_and_still_recognised() {
    let command = command_line(Path::new(r"C:\My Apps\demo.exe"), "hook");
    assert_eq!(command, r#""C:/My Apps/demo.exe" hook"#);
    assert!(runs(&command, "demo", "hook"));
}

#[test]
fn set_env_installs_skips_or_replaces_and_writes_the_file() {
    let cases = [
        (
            json!({"theme": "dark"}),
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            "200000",
            Step::Install,
            json!({"theme": "dark", "env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "200000"}}),
        ),
        (
            json!({"env": {"ANTHROPIC_BASE_URL": "http://x"}}),
            "ANTHROPIC_BASE_URL",
            "http://x",
            Step::Set,
            json!({"env": {"ANTHROPIC_BASE_URL": "http://x"}}),
        ),
        (
            json!({"env": {"ANTHROPIC_BASE_URL": "http://old"}}),
            "ANTHROPIC_BASE_URL",
            "http://new",
            Step::Replace,
            json!({"env": {"ANTHROPIC_BASE_URL": "http://new"}}),
        ),
    ];
    for (start, key, value, expected_step, expected) in cases {
        let mut v = start;
        let step = set_env(&mut v, key, value);
        assert_eq!(step, expected_step, "{key}={value}");
        assert_eq!(step.writes(), expected_step != Step::Set, "{key}={value}");
        assert_eq!(v, expected, "{key}={value}");
        assert_eq!(env_value(&v, key), Some(value), "{key}={value}");
    }

    let t = tempfile::tempdir().unwrap();
    let mut s = read(&t.path().join("settings.json")).unwrap();
    let step = set_env(&mut s.value, "ANTHROPIC_BASE_URL", "http://127.0.0.1:8080");
    assert_eq!(step, Step::Install);
    write(&s).unwrap();
    assert_eq!(
        written(&s.path)["env"],
        json!({"ANTHROPIC_BASE_URL": "http://127.0.0.1:8080"})
    );
    assert!(!t.path().join("settings.json.bak").exists());
}

#[test]
fn env_keys_and_settings_keep_their_order() {
    let mut v = json!({"model": "x", "env": {
        "FOO": "1", "ANTHROPIC_BASE_URL": "http://old", "BAR": "2"
    }, "theme": "dark"});
    set_env(&mut v, "ANTHROPIC_BASE_URL", "http://new");
    set_env(&mut v, "CLAUDE_CODE_MAX_CONTEXT_TOKENS", "200000");
    let top: Vec<_> = v.as_object().unwrap().keys().collect();
    assert_eq!(top, ["model", "env", "theme"]);
    let env: Vec<_> = v["env"].as_object().unwrap().keys().collect();
    assert_eq!(
        env,
        [
            "FOO",
            "ANTHROPIC_BASE_URL",
            "BAR",
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS"
        ]
    );
}

#[test]
fn remove_env_removes_only_the_key_with_the_expected_value() {
    let cases = [
        (
            json!({"env": {"ANTHROPIC_BASE_URL": "http://x", "FOO": "1"}}),
            Removal::Removed,
            json!({"env": {"FOO": "1"}}),
            true,
        ),
        (
            json!({"env": {"ANTHROPIC_BASE_URL": "http://hand-set"}}),
            Removal::Kept,
            json!({"env": {"ANTHROPIC_BASE_URL": "http://hand-set"}}),
            false,
        ),
        (
            json!({"theme": "dark", "env": {"FOO": "1"}}),
            Removal::Absent,
            json!({"theme": "dark", "env": {"FOO": "1"}}),
            false,
        ),
        (
            json!({"theme": "dark"}),
            Removal::Absent,
            json!({"theme": "dark"}),
            false,
        ),
        (
            json!({"theme": "dark", "env": {"ANTHROPIC_BASE_URL": "http://x"}}),
            Removal::Removed,
            json!({"theme": "dark"}),
            true,
        ),
    ];
    for (start, expected, expected_value, writes) in cases {
        let mut v = start.clone();
        let removal = remove_env(&mut v, "ANTHROPIC_BASE_URL", "http://x");
        assert_eq!(removal, expected);
        assert_eq!(removal.writes(), writes, "{start}");
        assert_eq!(v, expected_value, "{start}");
    }
}

#[test]
fn env_value_reads_only_string_values() {
    let v = json!({"env": {"ANTHROPIC_BASE_URL": "http://x", "CLAUDE_CODE_MAX_CONTEXT_TOKENS": 200000}});
    assert_eq!(env_value(&v, "ANTHROPIC_BASE_URL"), Some("http://x"));
    assert_eq!(env_value(&v, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"), None);
    assert_eq!(env_value(&v, "MISSING"), None);
    assert_eq!(env_value(&json!({}), "ANTHROPIC_BASE_URL"), None);
}
