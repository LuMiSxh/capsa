// SPDX-License-Identifier: MPL-2.0

use serde_json::{Value, json};

use super::spec::{add as add_missing_hook, state as hook_state};
use super::{Agent, HookFile, HookSpec, HookState, StatusLineSpec, StatusLineState};
use crate::claude_settings::{Step, runs};
use crate::render::Status;

const COMMAND: &str = "demo-hook run";

fn owned(command: &str) -> bool {
    runs(command, "demo-hook", "run") || runs(command, "demo-hook", "old")
}

fn legacy(command: &str) -> bool {
    runs(command, "demo-hook", "old")
}

const SPEC: HookSpec = HookSpec {
    event: "PreToolUse",
    matcher: Some("Bash"),
    command: COMMAND,
    owns: owned,
    legacy: Some(legacy),
};

const UNSCOPED: HookSpec = HookSpec {
    event: "SubagentStart",
    matcher: None,
    ..SPEC
};

fn entry(matcher: Option<&str>, command: &str) -> Value {
    let mut group = json!({"hooks": [{"type": "command", "command": command}]});
    if let Some(matcher) = matcher {
        group["matcher"] = json!(matcher);
    }
    group
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "callers pass `vec!` literals"
)]
fn settings(event: &str, entries: Vec<Value>) -> Value {
    json!({"hooks": {event: entries}})
}

#[test]
fn reports_missing_when_the_event_has_no_entry_of_the_tool() {
    assert_eq!(hook_state(&json!({}), &SPEC), HookState::Missing);
    let foreign = settings("PreToolUse", vec![entry(Some("Bash"), "other-tool run")]);
    assert_eq!(hook_state(&foreign, &SPEC), HookState::Missing);
}

#[test]
fn reports_installed_under_the_spec_matcher() {
    let v = settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)]);
    assert_eq!(hook_state(&v, &SPEC), HookState::Installed);
}

#[test]
fn reports_installed_without_a_matcher_when_the_spec_has_none() {
    let v = settings("SubagentStart", vec![entry(None, COMMAND)]);
    assert_eq!(hook_state(&v, &UNSCOPED), HookState::Installed);
}

#[test]
fn reports_a_custom_matcher_the_user_chose() {
    let v = settings("PreToolUse", vec![entry(Some("Read"), COMMAND)]);
    assert_eq!(
        hook_state(&v, &SPEC),
        HookState::CustomMatcher {
            matcher: Some("Read".to_owned())
        }
    );
}

#[test]
fn reports_a_custom_matcher_when_the_spec_has_none_and_the_entry_has_one() {
    let v = settings("SubagentStart", vec![entry(Some("Explore"), COMMAND)]);
    assert_eq!(
        hook_state(&v, &UNSCOPED),
        HookState::CustomMatcher {
            matcher: Some("Explore".to_owned())
        }
    );
}

#[test]
fn reports_a_custom_matcher_when_the_entry_has_none() {
    let v = settings("PreToolUse", vec![entry(None, COMMAND)]);
    assert_eq!(
        hook_state(&v, &SPEC),
        HookState::CustomMatcher { matcher: None }
    );
}

#[test]
fn reports_outdated_for_the_older_form_of_the_command() {
    let v = settings("PreToolUse", vec![entry(Some("Bash"), "demo-hook old")]);
    assert_eq!(
        hook_state(&v, &SPEC),
        HookState::Outdated {
            command: "demo-hook old".to_owned()
        }
    );
}

#[test]
fn reports_outdated_for_the_same_binary_with_other_arguments() {
    let v = settings(
        "PreToolUse",
        vec![entry(Some("Bash"), "demo-hook run --verbose")],
    );
    assert_eq!(
        hook_state(&v, &SPEC),
        HookState::Outdated {
            command: "demo-hook run --verbose".to_owned()
        }
    );
}

#[test]
fn reports_outdated_before_a_current_entry() {
    let v = settings(
        "PreToolUse",
        vec![
            entry(Some("Bash"), COMMAND),
            entry(Some("Bash"), "demo-hook old"),
        ],
    );
    assert_eq!(
        hook_state(&v, &SPEC),
        HookState::Outdated {
            command: "demo-hook old".to_owned()
        }
    );
}

#[test]
fn add_missing_installs_next_to_foreign_hooks() {
    let mut v = settings("PreToolUse", vec![entry(Some("Read"), "other-tool run")]);
    assert_eq!(add_missing_hook(&mut v, &SPEC), Step::Install);
    assert_eq!(hook_state(&v, &SPEC), HookState::Installed);
    assert_eq!(
        v["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "other-tool run"
    );
}

#[test]
fn add_missing_writes_the_hook_into_an_empty_object() {
    let mut v = json!({});
    assert_eq!(add_missing_hook(&mut v, &SPEC), Step::Install);
    assert_eq!(
        v,
        settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)])
    );
}

#[test]
fn add_missing_replaces_a_legacy_entry_and_keeps_foreign_hooks() {
    let mut v = json!({"hooks": {"PreToolUse": [{
        "matcher": "Bash",
        "hooks": [
            {"type": "command", "command": "demo-hook old"},
            {"type": "command", "command": "other-tool run"}
        ]
    }]}});
    assert_eq!(add_missing_hook(&mut v, &SPEC), Step::Replace);
    assert_eq!(hook_state(&v, &SPEC), HookState::Installed);
    assert_eq!(
        v["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "other-tool run"
    );
}

#[test]
fn add_missing_replaces_a_lone_legacy_entry_with_the_current_hook() {
    let mut v = settings("PreToolUse", vec![entry(Some("Bash"), "demo-hook old")]);
    assert_eq!(add_missing_hook(&mut v, &SPEC), Step::Replace);
    assert_eq!(
        v,
        settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)])
    );
}

#[test]
fn add_missing_keeps_a_custom_matcher_and_writes_nothing() {
    let mut v = settings("PreToolUse", vec![entry(Some("Read"), COMMAND)]);
    let before = v.clone();
    assert_eq!(add_missing_hook(&mut v, &SPEC), Step::Set);
    assert_eq!(v, before);
}

#[test]
fn add_missing_keeps_an_outdated_entry_that_is_not_legacy() {
    let mut v = settings(
        "PreToolUse",
        vec![entry(Some("Bash"), "demo-hook run --verbose")],
    );
    let before = v.clone();
    assert_eq!(add_missing_hook(&mut v, &SPEC), Step::Set);
    assert_eq!(v, before);
}

#[test]
fn add_missing_leaves_an_installed_hook_alone() {
    let mut v = settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)]);
    let before = v.clone();
    assert_eq!(add_missing_hook(&mut v, &SPEC), Step::Set);
    assert_eq!(v, before);
}

fn open(v: &Value) -> (tempfile::TempDir, HookFile) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, v.to_string()).unwrap();
    let file = HookFile::open_at(Agent::ClaudeCode, &path).unwrap();
    (dir, file)
}

fn saved(file: &HookFile) -> Value {
    serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap()
}

#[test]
fn opens_a_missing_file_as_an_empty_document() {
    let dir = tempfile::tempdir().unwrap();
    let file = HookFile::open_at(Agent::ClaudeCode, &dir.path().join("settings.json")).unwrap();
    assert_eq!(file.value(), &json!({}));
    assert_eq!(file.state(&SPEC), HookState::Missing);
}

#[test]
fn set_installs_then_reports_the_same_entry_as_set() {
    let (_dir, mut file) = open(&json!({"theme": "dark"}));
    assert_eq!(file.set(&SPEC), Step::Install);
    assert_eq!(file.set(&SPEC), Step::Set);
    file.write().unwrap();
    let saved = saved(&file);
    assert_eq!(saved["theme"], "dark");
    assert_eq!(
        saved["hooks"],
        settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)])["hooks"]
    );
}

#[test]
fn set_replaces_legacy_and_outdated_entries_with_the_canonical_one() {
    let v = settings(
        "PreToolUse",
        vec![
            entry(Some("Bash"), "demo-hook old"),
            entry(Some("Read"), "demo-hook run --verbose"),
        ],
    );
    let (_dir, mut file) = open(&v);
    assert_eq!(file.set(&SPEC), Step::Replace);
    assert_eq!(file.state(&SPEC), HookState::Installed);
    assert_eq!(
        file.value(),
        &settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)])
    );
}

#[test]
fn set_replaces_a_custom_matcher_when_the_spec_has_one() {
    let (_dir, mut file) = open(&settings("PreToolUse", vec![entry(Some("Read"), COMMAND)]));
    assert_eq!(file.set(&SPEC), Step::Replace);
    assert_eq!(file.state(&SPEC), HookState::Installed);
}

#[test]
fn set_keeps_a_custom_matcher_when_the_spec_has_none() {
    let v = settings("SubagentStart", vec![entry(Some("Explore"), COMMAND)]);
    let (_dir, mut file) = open(&v);
    assert_eq!(file.set(&UNSCOPED), Step::Set);
    assert_eq!(file.value(), &v);
}

#[test]
fn set_replaces_an_outdated_entry_even_when_the_spec_has_no_matcher() {
    let v = settings(
        "SubagentStart",
        vec![entry(Some("Explore"), "demo-hook old")],
    );
    let (_dir, mut file) = open(&v);
    assert_eq!(file.set(&UNSCOPED), Step::Replace);
    assert_eq!(file.state(&UNSCOPED), HookState::Installed);
}

#[test]
fn set_and_remove_leave_foreign_hooks_untouched() {
    let foreign = entry(Some("Bash"), "other-tool run");
    let (_dir, mut file) = open(&settings("PreToolUse", vec![foreign.clone()]));
    file.set(&SPEC);
    assert_eq!(file.value()["hooks"]["PreToolUse"][0], foreign);
    assert!(file.remove(&SPEC));
    assert_eq!(file.value(), &settings("PreToolUse", vec![foreign]));
}

#[test]
fn remove_claims_every_owned_entry_and_drops_the_empty_hooks() {
    let v = settings(
        "PreToolUse",
        vec![entry(Some("Bash"), COMMAND), entry(None, "demo-hook old")],
    );
    let (_dir, mut file) = open(&v);
    assert!(file.remove(&SPEC));
    assert_eq!(file.value(), &json!({}));
    assert!(!file.remove(&SPEC));
}

#[test]
fn add_through_the_file_keeps_a_custom_matcher_and_installs_a_missing_hook() {
    let v = settings("PreToolUse", vec![entry(Some("Read"), COMMAND)]);
    let (_dir, mut file) = open(&v);
    assert_eq!(file.add(&SPEC), Step::Set);
    assert_eq!(file.add(&UNSCOPED), Step::Install);
    assert_eq!(file.state(&UNSCOPED), HookState::Installed);
    assert_eq!(
        file.value()["hooks"]["PreToolUse"],
        v["hooks"]["PreToolUse"]
    );
}

#[test]
fn a_write_is_refused_when_the_file_changed_since_it_was_opened() {
    let (_dir, mut file) = open(&json!({}));
    file.set(&SPEC);
    std::fs::write(file.path(), "{\"other\": 1}").unwrap();
    assert!(file.write().is_err());
}

#[test]
fn checks_report_each_state_with_the_setup_fix() {
    let ok = HookState::Installed.check(&SPEC, "demo self setup");
    assert_eq!(
        (ok.status, ok.detail.as_str()),
        (Status::Ok, "matcher \"Bash\"")
    );
    assert_eq!(ok.name, "PreToolUse hook");
    assert!(ok.hint.is_none());

    let every = HookState::Installed.check(&UNSCOPED, "demo self setup");
    assert_eq!(every.detail, "every SubagentStart");

    let custom = HookState::CustomMatcher {
        matcher: Some("Read".to_owned()),
    }
    .check(&SPEC, "demo self setup");
    assert_eq!(
        (custom.status, custom.detail.as_str()),
        (Status::Ok, "matcher \"Read\"")
    );

    let outdated = HookState::Outdated {
        command: "demo-hook old".to_owned(),
    }
    .check(&SPEC, "demo self setup");
    assert_eq!(outdated.status, Status::Warn);
    assert_eq!(outdated.detail, "outdated: demo-hook old");
    assert_eq!(outdated.hint.as_deref(), Some("run demo self setup"));

    let missing = HookState::Missing.check(&SPEC, "demo self setup");
    assert_eq!(missing.status, Status::Warn);
    assert_eq!(missing.detail, "not registered");
    assert_eq!(missing.hint.as_deref(), Some("run demo self setup"));
}

const LINE: StatusLineSpec = StatusLineSpec {
    command: "demo-hook statusline",
    owns: owns_line,
};

fn owns_line(command: &str) -> bool {
    runs(command, "demo-hook", "statusline")
}

fn status_line(command: &str) -> Value {
    json!({"type": "command", "command": command})
}

#[test]
fn reports_each_status_line_state() {
    let (_dir, file) = open(&json!({}));
    assert_eq!(file.status_line(&LINE), StatusLineState::Missing);

    let (_dir, file) = open(&json!({"statusLine": status_line("demo-hook statusline")}));
    assert_eq!(file.status_line(&LINE), StatusLineState::Installed);

    let (_dir, file) = open(&json!({"statusLine": status_line("demo-hook statusline --wide")}));
    assert_eq!(
        file.status_line(&LINE),
        StatusLineState::Outdated {
            command: "demo-hook statusline --wide".to_owned()
        }
    );

    let (_dir, file) = open(&json!({"statusLine": status_line("other-tool")}));
    assert_eq!(
        file.status_line(&LINE),
        StatusLineState::Foreign {
            command: Some("other-tool".to_owned())
        }
    );

    let (_dir, file) = open(&json!({"statusLine": {"type": "command"}}));
    assert_eq!(
        file.status_line(&LINE),
        StatusLineState::Foreign { command: None }
    );
}

#[test]
fn set_status_line_installs_then_reports_the_same_line_as_set() {
    let (_dir, mut file) = open(&json!({"theme": "dark"}));
    assert_eq!(file.set_status_line(&LINE), Step::Install);
    assert_eq!(file.set_status_line(&LINE), Step::Set);
    file.write().unwrap();
    assert_eq!(
        saved(&file),
        json!({"theme": "dark", "statusLine": status_line("demo-hook statusline")})
    );

    let mut again = HookFile::open_at(Agent::ClaudeCode, file.path()).unwrap();
    assert_eq!(again.status_line(&LINE), StatusLineState::Installed);
    assert_eq!(again.set_status_line(&LINE), Step::Set);
}

#[test]
fn set_status_line_replaces_an_outdated_or_foreign_line() {
    let (_dir, mut file) = open(&json!({"statusLine": status_line("demo-hook statusline --wide")}));
    assert_eq!(file.set_status_line(&LINE), Step::Replace);
    assert_eq!(file.status_line(&LINE), StatusLineState::Installed);

    let (_dir, mut file) = open(&json!({"statusLine": status_line("other-tool")}));
    assert_eq!(file.set_status_line(&LINE), Step::Replace);
    assert_eq!(
        file.value()["statusLine"],
        status_line("demo-hook statusline")
    );
}

#[test]
fn remove_status_line_drops_the_tools_line_and_keeps_a_foreign_one() {
    let (_dir, mut file) = open(&json!({
        "theme": "dark",
        "statusLine": status_line("demo-hook statusline --wide"),
    }));
    assert!(file.remove_status_line(&LINE));
    assert_eq!(file.value(), &json!({"theme": "dark"}));
    assert!(!file.remove_status_line(&LINE));

    let (_dir, mut file) = open(&json!({"statusLine": status_line("other-tool")}));
    assert!(!file.remove_status_line(&LINE));
    assert_eq!(file.value()["statusLine"], status_line("other-tool"));
}
