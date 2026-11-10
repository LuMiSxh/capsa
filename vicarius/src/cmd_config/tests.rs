// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::time::Duration;

use clap::Parser;
use serde_json::{Value, json};
use vicarius::agents::{Entry, Outcome};
use vicarius::config::{Config, KNOWN_MODELS, known_model};

use super::{Action, AgentAction, AgentArgs, Key, TargetArgs, apply_steps, sync_agents, sync_step};
use crate::cmd_service::Item;
use crate::service::{Install, base_url};
use crate::test_support::{FakeLaunchctl, free_port, install_in, serve};

const STATUS: &str = r#"{"started_at":1,"gate":{"open":true}}"#;
const WAIT: Duration = Duration::from_secs(2);

#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    action: Action,
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn write_env(install: &Install, env: &Value) {
    std::fs::write(&install.settings, json!({ "env": env }).to_string()).unwrap();
}

fn status_of(items: &[Item], name: &str) -> String {
    items
        .iter()
        .find(|item| item.name == name)
        .map(|item| item.status.clone())
        .unwrap_or_default()
}

#[test]
fn suspend_and_activate_take_a_name_or_all_but_not_both() {
    let parsed = TestCli::try_parse_from(["vicarius", "agent", "suspend", "reviewer"]).unwrap();
    assert!(matches!(
        &parsed.action,
        Action::Agent(AgentArgs {
            action: AgentAction::Suspend(TargetArgs {
                from: Some(from),
                all: false,
            }),
        }) if from.as_str() == "reviewer"
    ));

    let parsed = TestCli::try_parse_from(["vicarius", "agent", "activate", "--all"]).unwrap();
    assert!(matches!(
        &parsed.action,
        Action::Agent(AgentArgs {
            action: AgentAction::Activate(TargetArgs {
                from: None,
                all: true,
            }),
        })
    ));

    assert!(TestCli::try_parse_from(["vicarius", "agent", "suspend"]).is_err());
    assert!(
        TestCli::try_parse_from(["vicarius", "agent", "suspend", "reviewer", "--all"]).is_err()
    );
}

#[test]
fn a_stopped_service_skips_the_sync_and_notes_when_the_files_arrive() {
    let config = Config {
        port: free_port(),
        ..Config::default()
    };
    let (steps, note) = sync_agents(&config);
    assert!(steps.is_empty());
    assert!(note.unwrap().contains("written when it starts"));
}

#[test]
fn a_sync_entry_becomes_a_named_step() {
    let written = Entry {
        agent: "reviewer".to_owned(),
        file: "reviewer-ds.md".to_owned(),
        outcome: Outcome::Written,
    };
    let item = sync_step(&written);
    assert_eq!(item.name, "reviewer-ds");
    assert_eq!(item.status, "written");

    let missing = Entry {
        outcome: Outcome::SourceMissing,
        ..written
    };
    assert_eq!(sync_step(&missing).status, "source missing");
}

#[test]
fn the_known_models_cover_the_documented_ids() {
    assert!(known_model("deepseek-flash"));
    assert!(known_model("deepseek-v4-pro"));
    assert!(!known_model("deepseek-next"));
    assert_eq!(KNOWN_MODELS, &["deepseek-flash", "deepseek-v4-pro"]);
}

#[test]
fn max_context_replaces_the_value_vicarius_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    write_env(
        &install,
        &json!({"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "400000", "OTHER": "kept"}),
    );
    let new = Config {
        max_context_tokens: 500_000,
        ..Config::default()
    };
    let items = apply_steps(
        &install,
        &FakeLaunchctl::default(),
        Key::MaxContext,
        &Config::default(),
        &new,
        WAIT,
    )
    .unwrap();
    assert_eq!(
        status_of(&items, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
        "replaced"
    );
    assert_eq!(
        read_json(&install.settings),
        json!({"env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "500000", "OTHER": "kept"}})
    );
}

#[test]
fn max_context_keeps_a_value_set_by_hand() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    write_env(&install, &json!({"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"}));
    let new = Config {
        max_context_tokens: 500_000,
        ..Config::default()
    };
    let items = apply_steps(
        &install,
        &FakeLaunchctl::default(),
        Key::MaxContext,
        &Config::default(),
        &new,
        WAIT,
    )
    .unwrap();
    assert_eq!(status_of(&items, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"), "kept");
    assert_eq!(
        read_json(&install.settings),
        json!({"env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"}})
    );
}

#[test]
fn port_restarts_a_loaded_agent_and_rewrites_the_url_once_it_answers() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let old = Config {
        port: free_port(),
        ..Config::default()
    };
    let new = Config {
        port: serve(vec![("GET /vicarius/status", 200, STATUS)]),
        ..Config::default()
    };
    write_env(&install, &json!({"ANTHROPIC_BASE_URL": base_url(old.port)}));
    let lctl = FakeLaunchctl::default();
    lctl.loaded.set(true);
    let items = apply_steps(&install, &lctl, Key::Port, &old, &new, WAIT).unwrap();
    assert_eq!(status_of(&items, "agent"), "reloaded");
    assert_eq!(status_of(&items, "ANTHROPIC_BASE_URL"), "replaced");
    assert_eq!(
        read_json(&install.settings),
        json!({"env": {"ANTHROPIC_BASE_URL": base_url(new.port)}})
    );
    let calls = lctl.calls.borrow();
    let bootout = calls.iter().position(|c| c.starts_with("bootout"));
    let bootstrap = calls.iter().position(|c| c.starts_with("bootstrap"));
    assert!(
        matches!((bootout, bootstrap), (Some(out), Some(up)) if out < up),
        "{calls:?}"
    );
}

#[test]
fn port_that_does_not_answer_leaves_the_settings_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let old = Config {
        port: free_port(),
        ..Config::default()
    };
    let new = Config {
        port: free_port(),
        ..Config::default()
    };
    write_env(&install, &json!({"ANTHROPIC_BASE_URL": base_url(old.port)}));
    let before = std::fs::read_to_string(&install.settings).unwrap();
    let lctl = FakeLaunchctl::default();
    lctl.loaded.set(true);
    let err = apply_steps(
        &install,
        &lctl,
        Key::Port,
        &old,
        &new,
        Duration::from_millis(300),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("does not answer"), "{err:#}");
    assert_eq!(
        capsa_core::hint::find_hint(&err)
            .map(|h| h.contains("service.log") && h.contains("vicarius self doctor")),
        Some(true)
    );
    assert_eq!(std::fs::read_to_string(&install.settings).unwrap(), before);
}

#[test]
fn port_with_the_agent_not_loaded_rewrites_the_url_without_touching_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let old = Config {
        port: free_port(),
        ..Config::default()
    };
    let new = Config {
        port: free_port(),
        ..Config::default()
    };
    write_env(&install, &json!({"ANTHROPIC_BASE_URL": base_url(old.port)}));
    let lctl = FakeLaunchctl::default();
    let items = apply_steps(&install, &lctl, Key::Port, &old, &new, WAIT).unwrap();
    assert_eq!(status_of(&items, "ANTHROPIC_BASE_URL"), "replaced");
    assert_eq!(
        read_json(&install.settings),
        json!({"env": {"ANTHROPIC_BASE_URL": base_url(new.port)}})
    );
    assert!(lctl.calls.borrow().iter().all(|c| c.starts_with("print")));
}

#[test]
fn port_with_the_agent_not_loaded_creates_no_settings_when_the_url_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let old = Config {
        port: free_port(),
        ..Config::default()
    };
    let new = Config {
        port: free_port(),
        ..Config::default()
    };
    let items = apply_steps(
        &install,
        &FakeLaunchctl::default(),
        Key::Port,
        &old,
        &new,
        WAIT,
    )
    .unwrap();
    assert_eq!(status_of(&items, "ANTHROPIC_BASE_URL"), "absent");
    assert!(!install.settings.exists());
}

#[test]
fn a_port_that_does_not_answer_is_rolled_back_to_the_old_port() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let old = Config {
        port: serve(vec![("GET /vicarius/status", 200, STATUS)]),
        ..Config::default()
    };
    let new = Config {
        port: free_port(),
        ..Config::default()
    };
    old.save_to(&install.config).unwrap();
    write_env(&install, &json!({"ANTHROPIC_BASE_URL": base_url(old.port)}));
    let before = std::fs::read_to_string(&install.settings).unwrap();
    let lctl = FakeLaunchctl::default();
    lctl.loaded.set(true);
    let err = apply_steps(
        &install,
        &lctl,
        Key::Port,
        &old,
        &new,
        Duration::from_millis(300),
    )
    .unwrap_err();
    let text = format!("{err:#}");
    assert!(
        text.contains("was rolled back: config.json is back on port"),
        "{text}"
    );
    assert_eq!(Config::load_from(&install.config).unwrap().port, old.port);
    assert_eq!(std::fs::read_to_string(&install.settings).unwrap(), before);
    let calls = lctl.calls.borrow();
    assert_eq!(
        calls.iter().filter(|c| c.starts_with("bootstrap")).count(),
        2,
        "{calls:?}"
    );
    assert!(
        capsa_core::hint::find_hint(&err).is_some_and(
            |hint| hint.contains("service.log") && hint.contains("vicarius self doctor")
        )
    );
}
