// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use serde_json::{Value, json};

use super::{Step, command_line, has_hook, read, runs, set_hook, write};

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
