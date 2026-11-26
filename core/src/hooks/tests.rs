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
fn hook_state_reports_every_relation_to_the_spec() {
    let cases = [
        (json!({}), SPEC, HookState::Missing),
        (
            settings("PreToolUse", vec![entry(Some("Bash"), "other-tool run")]),
            SPEC,
            HookState::Missing,
        ),
        (
            settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)]),
            SPEC,
            HookState::Installed,
        ),
        (
            settings("SubagentStart", vec![entry(None, COMMAND)]),
            UNSCOPED,
            HookState::Installed,
        ),
        (
            settings("PreToolUse", vec![entry(Some("Read"), COMMAND)]),
            SPEC,
            HookState::CustomMatcher {
                matcher: Some("Read".to_owned()),
            },
        ),
        (
            settings("SubagentStart", vec![entry(Some("Explore"), COMMAND)]),
            UNSCOPED,
            HookState::CustomMatcher {
                matcher: Some("Explore".to_owned()),
            },
        ),
        (
            settings("PreToolUse", vec![entry(None, COMMAND)]),
            SPEC,
            HookState::CustomMatcher { matcher: None },
        ),
        (
            settings("PreToolUse", vec![entry(Some("Bash"), "demo-hook old")]),
            SPEC,
            HookState::Outdated {
                command: "demo-hook old".to_owned(),
            },
        ),
        (
            settings(
                "PreToolUse",
                vec![entry(Some("Bash"), "demo-hook run --verbose")],
            ),
            SPEC,
            HookState::Outdated {
                command: "demo-hook run --verbose".to_owned(),
            },
        ),
        (
            settings(
                "PreToolUse",
                vec![
                    entry(Some("Bash"), COMMAND),
                    entry(Some("Bash"), "demo-hook old"),
                ],
            ),
            SPEC,
            HookState::Outdated {
                command: "demo-hook old".to_owned(),
            },
        ),
    ];
    for (value, spec, expected) in cases {
        assert_eq!(hook_state(&value, &spec), expected);
    }
}

#[test]
fn add_installs_replaces_or_keeps_the_hook_and_its_neighbours() {
    let unchanged = |v: Value| (v.clone(), Step::Set, v, None::<HookState>);
    let cases = vec![
        (
            settings("PreToolUse", vec![entry(Some("Read"), "other-tool run")]),
            Step::Install,
            settings(
                "PreToolUse",
                vec![
                    entry(Some("Read"), "other-tool run"),
                    entry(Some("Bash"), COMMAND),
                ],
            ),
            Some(HookState::Installed),
        ),
        (
            json!({}),
            Step::Install,
            settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)]),
            None,
        ),
        (
            json!({"hooks": {"PreToolUse": [{
                "matcher": "Bash",
                "hooks": [
                    {"type": "command", "command": "demo-hook old"},
                    {"type": "command", "command": "other-tool run"}
                ]
            }]}}),
            Step::Replace,
            json!({"hooks": {"PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "other-tool run"}]},
                {"matcher": "Bash", "hooks": [{"type": "command", "command": COMMAND}]}
            ]}}),
            Some(HookState::Installed),
        ),
        (
            settings("PreToolUse", vec![entry(Some("Bash"), "demo-hook old")]),
            Step::Replace,
            settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)]),
            None,
        ),
        unchanged(settings("PreToolUse", vec![entry(Some("Read"), COMMAND)])),
        unchanged(settings(
            "PreToolUse",
            vec![entry(Some("Bash"), "demo-hook run --verbose")],
        )),
        unchanged(settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)])),
    ];
    for (mut value, expected_step, expected, state) in cases {
        assert_eq!(add_missing_hook(&mut value, &SPEC), expected_step);
        assert_eq!(value, expected);
        if let Some(state) = state {
            assert_eq!(hook_state(&value, &SPEC), state);
        }
    }
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
fn set_installs_or_keeps_an_entry_and_reports_the_step() {
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

    let v = settings("SubagentStart", vec![entry(Some("Explore"), COMMAND)]);
    let (_dir, mut file) = open(&v);
    assert_eq!(file.set(&UNSCOPED), Step::Set);
    assert_eq!(file.value(), &v);
}

#[test]
fn set_replaces_a_legacy_outdated_or_custom_matcher_entry() {
    let canonical = settings("PreToolUse", vec![entry(Some("Bash"), COMMAND)]);
    let cases = [
        (
            settings(
                "PreToolUse",
                vec![
                    entry(Some("Bash"), "demo-hook old"),
                    entry(Some("Read"), "demo-hook run --verbose"),
                ],
            ),
            SPEC,
            Some(canonical),
        ),
        (
            settings("PreToolUse", vec![entry(Some("Read"), COMMAND)]),
            SPEC,
            None,
        ),
        (
            settings(
                "SubagentStart",
                vec![entry(Some("Explore"), "demo-hook old")],
            ),
            UNSCOPED,
            None,
        ),
    ];
    for (initial, spec, expected_value) in cases {
        let (_dir, mut file) = open(&initial);
        assert_eq!(file.set(&spec), Step::Replace);
        assert_eq!(file.state(&spec), HookState::Installed);
        if let Some(expected) = expected_value {
            assert_eq!(file.value(), &expected);
        }
    }
}

#[test]
fn remove_claims_every_owned_entry_and_keeps_foreign_hooks() {
    let foreign = entry(Some("Bash"), "other-tool run");
    let (_dir, mut file) = open(&settings("PreToolUse", vec![foreign.clone()]));
    file.set(&SPEC);
    assert_eq!(file.value()["hooks"]["PreToolUse"][0], foreign);
    assert!(file.remove(&SPEC));
    assert_eq!(file.value(), &settings("PreToolUse", vec![foreign]));

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
fn set_status_line_installs_or_replaces_the_line() {
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

    for (initial, expected) in [
        (
            json!({"statusLine": status_line("demo-hook statusline --wide")}),
            None,
        ),
        (
            json!({"statusLine": status_line("other-tool")}),
            Some(status_line("demo-hook statusline")),
        ),
    ] {
        let (_dir, mut file) = open(&initial);
        assert_eq!(file.set_status_line(&LINE), Step::Replace);
        assert_eq!(file.status_line(&LINE), StatusLineState::Installed);
        if let Some(expected) = expected {
            assert_eq!(file.value()["statusLine"], expected);
        }
    }
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
