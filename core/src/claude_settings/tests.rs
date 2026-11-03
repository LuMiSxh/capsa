// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use serde_json::{Value, json};

use crate::hint::Hint;

use super::{
    Removal, Step, command_line, env_value, has_hook, read, remove_env, remove_hook, runs, set_env,
    set_hook, write,
};

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
fn adds_the_hook_to_an_empty_object() {
    let mut v = json!({});
    let step = set_hook(&mut v, "PreToolUse", None, COMMAND, owned);
    assert_eq!(step, Step::Install);
    assert_eq!(
        v["hooks"]["PreToolUse"],
        json!([{"hooks": [{"type": "command", "command": COMMAND}]}])
    );
    assert_eq!(has_hook(&v, "PreToolUse", owned), Some(""));
}

#[test]
fn replaces_own_hook_with_a_different_matcher() {
    let mut v = json!({"hooks": {"SessionStart": [
        {"matcher": "startup", "hooks": [{"type": "command", "command": COMMAND}]}
    ]}});
    let step = set_hook(
        &mut v,
        "SessionStart",
        Some("startup|clear"),
        COMMAND,
        owned,
    );
    assert_eq!(step, Step::Replace);
    let groups = v["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["matcher"], "startup|clear");
    assert_eq!(has_hook(&v, "SessionStart", owned), Some("startup|clear"));
}

#[test]
fn skips_the_write_when_the_hook_is_identical() {
    let start = json!({"hooks": {"SessionStart": [
        {"matcher": "startup", "hooks": [{"type": "command", "command": COMMAND}]}
    ]}});
    let mut v = start.clone();
    let step = set_hook(&mut v, "SessionStart", Some("startup"), COMMAND, owned);
    assert_eq!(step, Step::Set);
    assert!(!step.writes());
    assert_eq!(v, start);
}

#[test]
fn keeps_foreign_hooks_and_unrelated_settings_in_order() {
    let mut v = json!({"model": "x", "hooks": {"PreToolUse": [
        {"matcher": "Edit", "hooks": [{"type": "command", "command": "fmt.sh"}]}
    ], "Stop": [{"hooks": []}]}, "theme": "dark"});
    set_hook(&mut v, "PreToolUse", Some("Bash"), COMMAND, owned);
    assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "fmt.sh");
    assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    assert_eq!(v["hooks"]["Stop"], json!([{"hooks": []}]));
    let keys: Vec<_> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["model", "hooks", "theme"]);
}

#[test]
fn replacing_an_entry_keeps_the_foreign_item_in_its_group() {
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
fn refuses_a_file_that_changed_since_it_was_read() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("settings.json");
    std::fs::write(&path, r#"{"theme":"light"}"#).unwrap();
    let mut s = read(&path).unwrap();
    set_hook(&mut s.value, "PreToolUse", None, COMMAND, owned);
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

#[test]
fn a_write_keeps_the_old_file_as_bak() {
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
fn read_refuses_a_non_object_and_wrong_hooks_shapes() {
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
fn remove_hook_keeps_foreign_hooks_and_reports_absence() {
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
}

#[test]
fn removing_the_only_hook_drops_the_empty_event_and_hooks() {
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
}

#[cfg(not(windows))]
#[test]
fn a_binary_path_with_a_space_is_quoted() {
    assert_eq!(
        command_line(Path::new("/usr/bin/demo"), "hook"),
        "/usr/bin/demo hook"
    );
    assert_eq!(
        command_line(Path::new("/My Apps/demo"), "hook"),
        "\"/My Apps/demo\" hook"
    );
}

#[cfg(windows)]
#[test]
fn a_windows_binary_path_is_quoted_with_forward_slashes_and_still_recognised() {
    let command = command_line(Path::new(r"C:\My Apps\demo.exe"), "hook");
    assert_eq!(command, r#""C:/My Apps/demo.exe" hook"#);
    assert!(runs(&command, "demo", "hook"));
}

#[test]
fn sets_an_env_key_on_a_missing_file() {
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
fn adds_env_to_a_file_without_it_and_keeps_the_rest() {
    let mut v = json!({"theme": "dark"});
    let step = set_env(&mut v, "CLAUDE_CODE_MAX_CONTEXT_TOKENS", "200000");
    assert_eq!(step, Step::Install);
    assert_eq!(
        v,
        json!({"theme": "dark", "env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "200000"}})
    );
}

#[test]
fn skips_the_write_when_the_env_value_is_identical() {
    let start = json!({"env": {"ANTHROPIC_BASE_URL": "http://x"}});
    let mut v = start.clone();
    let step = set_env(&mut v, "ANTHROPIC_BASE_URL", "http://x");
    assert_eq!(step, Step::Set);
    assert!(!step.writes());
    assert_eq!(v, start);
}

#[test]
fn replaces_an_env_value_that_differs() {
    let mut v = json!({"env": {"ANTHROPIC_BASE_URL": "http://old"}});
    let step = set_env(&mut v, "ANTHROPIC_BASE_URL", "http://new");
    assert_eq!(step, Step::Replace);
    assert_eq!(env_value(&v, "ANTHROPIC_BASE_URL"), Some("http://new"));
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
fn removes_an_env_key_with_the_expected_value() {
    let mut v = json!({"env": {"ANTHROPIC_BASE_URL": "http://x", "FOO": "1"}});
    let removal = remove_env(&mut v, "ANTHROPIC_BASE_URL", "http://x");
    assert_eq!(removal, Removal::Removed);
    assert!(removal.writes());
    assert_eq!(v, json!({"env": {"FOO": "1"}}));
}

#[test]
fn keeps_an_env_key_whose_value_differs() {
    let start = json!({"env": {"ANTHROPIC_BASE_URL": "http://hand-set"}});
    let mut v = start.clone();
    let removal = remove_env(&mut v, "ANTHROPIC_BASE_URL", "http://x");
    assert_eq!(removal, Removal::Kept);
    assert!(!removal.writes());
    assert_eq!(v, start);
}

#[test]
fn removing_an_absent_env_key_changes_nothing() {
    let start = json!({"theme": "dark", "env": {"FOO": "1"}});
    let mut v = start.clone();
    assert_eq!(
        remove_env(&mut v, "ANTHROPIC_BASE_URL", "http://x"),
        Removal::Absent
    );
    assert_eq!(v, start);
    let mut none = json!({"theme": "dark"});
    assert_eq!(
        remove_env(&mut none, "ANTHROPIC_BASE_URL", "http://x"),
        Removal::Absent
    );
    assert_eq!(none, json!({"theme": "dark"}));
}

#[test]
fn removing_the_last_env_key_drops_the_env_object() {
    let mut v = json!({"theme": "dark", "env": {"ANTHROPIC_BASE_URL": "http://x"}});
    let removal = remove_env(&mut v, "ANTHROPIC_BASE_URL", "http://x");
    assert_eq!(removal, Removal::Removed);
    assert_eq!(v, json!({"theme": "dark"}));
}

#[test]
fn read_refuses_a_non_object_env() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("settings.json");
    std::fs::write(&path, r#"{"env": ["ANTHROPIC_BASE_URL"]}"#).unwrap();
    let err = read(&path).unwrap_err();
    assert!(err.to_string().contains("env in"), "{err}");
    assert!(err.hint().is_some());
}

#[test]
fn an_env_write_refuses_a_file_that_changed_since_it_was_read() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("settings.json");
    std::fs::write(&path, r#"{"theme":"light"}"#).unwrap();
    let mut s = read(&path).unwrap();
    set_env(&mut s.value, "ANTHROPIC_BASE_URL", "http://x");
    std::fs::write(&path, r#"{"theme":"dark"}"#).unwrap();

    let err = write(&s).unwrap_err();
    assert!(err.to_string().contains("changed while writing"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"theme":"dark"}"#
    );
}

#[test]
fn env_value_reads_only_string_values() {
    let v = json!({"env": {"ANTHROPIC_BASE_URL": "http://x", "CLAUDE_CODE_MAX_CONTEXT_TOKENS": 200000}});
    assert_eq!(env_value(&v, "ANTHROPIC_BASE_URL"), Some("http://x"));
    assert_eq!(env_value(&v, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"), None);
    assert_eq!(env_value(&v, "MISSING"), None);
    assert_eq!(env_value(&json!({}), "ANTHROPIC_BASE_URL"), None);
}
