// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use vicarius::config::Config;

use super::{
    Item, Report, remove_integration_with, setup_report, start_report, start_steps, stop_steps,
    wait_unloaded,
};
use crate::service::{Install, base_url, set_session_hook};
use crate::test_support::{FakeLaunchctl, free_port, install_in, serve};

const STATUS: &str = r#"{"started_at":1,"gate":{"open":true}}"#;

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn answering(routes: Vec<(&'static str, u16, &'static str)>) -> Config {
    Config {
        port: serve(routes),
        ..Config::default()
    }
}

fn status_of(items: &[Item], name: &str) -> String {
    items
        .iter()
        .find(|item| item.name == name)
        .map(|item| item.status.clone())
        .unwrap_or_default()
}

#[test]
fn start_sets_the_settings_keys_once_the_status_answers() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = answering(vec![("GET /vicarius/status", 200, STATUS)]);
    let lctl = FakeLaunchctl::default();
    let items = start_steps(&install, &lctl, &config, Duration::from_secs(2), false).unwrap();
    assert_eq!(status_of(&items, "agent"), "started");
    assert_eq!(
        status_of(&items, "status"),
        format!("answers on 127.0.0.1:{}", config.port)
    );
    assert_eq!(status_of(&items, "refresh"), "not available in this build");
    assert_eq!(status_of(&items, "ANTHROPIC_BASE_URL"), "installed");
    let settings = read_json(&install.settings);
    assert_eq!(
        settings["env"]["ANTHROPIC_BASE_URL"],
        format!("http://127.0.0.1:{}", config.port)
    );
    assert!(
        lctl.calls
            .borrow()
            .iter()
            .any(|c| c.starts_with("bootstrap gui/501 "))
    );
}

#[test]
fn start_writes_no_settings_keys_when_the_status_never_answers() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = Config {
        port: free_port(),
        ..Config::default()
    };
    let lctl = FakeLaunchctl::default();
    let err = start_steps(&install, &lctl, &config, Duration::from_millis(300), false).unwrap_err();
    assert!(format!("{err:#}").contains("does not answer"), "{err:#}");
    assert_eq!(
        capsa_core::hint::find_hint(&err).map(|h| h.contains("service.log")),
        Some(true)
    );
    assert!(!install.settings.exists());
}

#[test]
fn start_kickstarts_a_loaded_agent_instead_of_bootstrapping_it() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = answering(vec![("GET /vicarius/status", 200, STATUS)]);
    let lctl = FakeLaunchctl::default();
    lctl.loaded.set(true);
    let items = start_steps(&install, &lctl, &config, Duration::from_secs(2), false).unwrap();
    assert_eq!(status_of(&items, "agent"), "restarted");
    assert!(
        lctl.calls
            .borrow()
            .iter()
            .any(|c| c == "kickstart -k gui/501/io.github.lumisxh.vicarius")
    );
}

#[test]
fn setup_installs_the_plist_and_hook_once_and_starts_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = answering(vec![("GET /vicarius/status", 200, STATUS)]);
    let lctl = FakeLaunchctl::default();
    let first = setup_report(&install, &lctl, &config, false).unwrap();
    assert_eq!(status_of(&first.steps, "plist"), "written");
    assert_eq!(status_of(&first.steps, "SessionStart hook"), "installed");
    let second = setup_report(&install, &lctl, &config, false).unwrap();
    assert_eq!(status_of(&second.steps, "plist"), "unchanged");
    assert_eq!(status_of(&second.steps, "SessionStart hook"), "already set");
    assert_eq!(status_of(&second.steps, "agent"), "restarted");
    assert_eq!(
        status_of(&second.steps, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
        "already set"
    );
}

#[test]
fn setup_dry_run_writes_and_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let lctl = FakeLaunchctl::default();
    let report: Report = setup_report(&install, &lctl, &Config::default(), true).unwrap();
    assert_eq!(status_of(&report.steps, "plist"), "would write");
    assert_eq!(
        status_of(&report.steps, "SessionStart hook"),
        "would install"
    );
    assert!(report.dry_run);
    assert!(report.note.is_none());
    assert!(!install.plist.exists());
    assert!(!install.settings.exists());
    assert!(lctl.calls.borrow().is_empty());
}

#[test]
fn stop_removes_only_the_values_the_service_set_and_unloads_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    std::fs::write(
        &install.settings,
        json!({"env": {
            "ANTHROPIC_BASE_URL": "http://127.0.0.1:3337",
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"
        }})
        .to_string(),
    )
    .unwrap();
    let lctl = FakeLaunchctl::default();
    lctl.loaded.set(true);
    let items = stop_steps(&install, &lctl, &Config::default()).unwrap();
    assert_eq!(status_of(&items, "ANTHROPIC_BASE_URL"), "removed");
    assert_eq!(status_of(&items, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"), "kept");
    assert_eq!(status_of(&items, "agent"), "stopped");
    assert_eq!(
        read_json(&install.settings),
        json!({"env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"}})
    );
    assert!(
        lctl.calls
            .borrow()
            .iter()
            .any(|c| c.starts_with("bootout gui/501/"))
    );
}

#[test]
fn stop_is_not_an_error_when_the_agent_is_not_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let lctl = FakeLaunchctl::default();
    let items = stop_steps(&install, &lctl, &Config::default()).unwrap();
    assert_eq!(status_of(&items, "agent"), "not loaded");
    assert!(!lctl.calls.borrow().iter().any(|c| c.starts_with("bootout")));
}

#[test]
fn uninstall_removes_the_hook_the_plist_and_the_settings_keys() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = answering(vec![("GET /vicarius/status", 200, STATUS)]);
    let lctl = FakeLaunchctl::default();
    setup_report(&install, &lctl, &config, false).unwrap();
    assert!(install.plist.exists());
    remove_integration_with(&install, &lctl, &config).unwrap();
    assert!(!install.plist.exists());
    assert_eq!(read_json(&install.settings), json!({}));
}

#[test]
fn uninstall_is_quiet_when_nothing_was_installed() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let lctl = FakeLaunchctl::default();
    remove_integration_with(&install, &lctl, &Config::default()).unwrap();
    assert!(!install.settings.exists());
}

#[test]
fn start_reloads_a_loaded_agent_whose_property_list_changed() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = answering(vec![("GET /vicarius/status", 200, STATUS)]);
    let lctl = FakeLaunchctl::default();
    lctl.loaded.set(true);
    let items = start_steps(&install, &lctl, &config, Duration::from_secs(2), true).unwrap();
    assert_eq!(status_of(&items, "agent"), "reloaded");
    let calls = lctl.calls.borrow();
    let bootout = calls.iter().position(|c| c.starts_with("bootout"));
    let bootstrap = calls.iter().position(|c| c.starts_with("bootstrap"));
    assert!(
        matches!((bootout, bootstrap), (Some(out), Some(up)) if out < up),
        "{calls:?}"
    );
}

#[test]
fn start_refuses_a_hand_set_base_url_before_anything_is_written_or_launched() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let before = json!({"env": {"ANTHROPIC_BASE_URL": "https://gateway.example"}});
    std::fs::write(&install.settings, before.to_string()).unwrap();
    let config = Config {
        port: free_port(),
        ..Config::default()
    };
    let lctl = FakeLaunchctl::default();
    let err = start_report(&install, &lctl, &config).unwrap_err();
    assert!(
        format!("{err:#}").contains("ANTHROPIC_BASE_URL is set to"),
        "{err:#}"
    );
    assert!(capsa_core::hint::find_hint(&err).is_some_and(|hint| hint.contains("settings.json")));
    assert!(!install.plist.exists());
    assert_eq!(read_json(&install.settings), before);
    assert!(lctl.calls.borrow().is_empty());
}

#[test]
fn setup_refuses_a_hand_set_max_context_before_the_hook_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let before = json!({"env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"}});
    std::fs::write(&install.settings, before.to_string()).unwrap();
    let lctl = FakeLaunchctl::default();
    let err = setup_report(&install, &lctl, &Config::default(), false).unwrap_err();
    assert!(
        format!("{err:#}").contains("CLAUDE_CODE_MAX_CONTEXT_TOKENS is set to"),
        "{err:#}"
    );
    assert!(!install.plist.exists());
    assert_eq!(read_json(&install.settings), before);
    assert!(lctl.calls.borrow().is_empty());
}

#[test]
fn start_accepts_a_base_url_that_already_points_at_the_service() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = answering(vec![("GET /vicarius/status", 200, STATUS)]);
    std::fs::write(
        &install.settings,
        json!({"env": {"ANTHROPIC_BASE_URL": base_url(config.port)}}).to_string(),
    )
    .unwrap();
    let report = start_report(&install, &FakeLaunchctl::default(), &config).unwrap();
    assert_eq!(status_of(&report.steps, "agent"), "started");
    assert_eq!(
        status_of(&report.steps, "ANTHROPIC_BASE_URL"),
        "already set"
    );
    assert_eq!(
        status_of(&report.steps, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
        "installed"
    );
}

#[test]
fn launch_bootstraps_only_after_launchd_has_unloaded_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = answering(vec![("GET /vicarius/status", 200, STATUS)]);
    let lctl = FakeLaunchctl::default();
    lctl.loaded.set(true);
    lctl.linger.set(2);
    let items = start_steps(&install, &lctl, &config, Duration::from_secs(2), true).unwrap();
    assert_eq!(status_of(&items, "agent"), "reloaded");
    let calls = lctl.calls.borrow();
    let bootout = calls.iter().position(|c| c.starts_with("bootout")).unwrap();
    let bootstrap = calls
        .iter()
        .position(|c| c.starts_with("bootstrap"))
        .unwrap();
    let polls = &calls[bootout + 1..bootstrap];
    assert_eq!(polls.len(), 3, "{calls:?}");
    assert!(polls.iter().all(|c| c.starts_with("print")), "{calls:?}");
}

#[test]
fn a_loaded_agent_that_stays_loaded_fails_with_a_hint() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let lctl = FakeLaunchctl::default();
    lctl.loaded.set(true);
    let err = wait_unloaded(
        &install,
        &lctl,
        Duration::from_millis(250),
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("still has the agent loaded"),
        "{err:#}"
    );
    assert!(capsa_core::hint::find_hint(&err).is_some());
}

#[test]
fn uninstall_off_macos_removes_the_keys_and_the_hook_and_never_runs_launchd_or_id() {
    let dir = tempfile::tempdir().unwrap();
    let install = Install {
        plist: dir
            .path()
            .join("LaunchAgents/io.github.lumisxh.vicarius.plist"),
        settings: dir.path().join("settings.json"),
        ..Install::for_platform(false, || unreachable!("`id -u` must not run off macOS")).unwrap()
    };
    std::fs::create_dir_all(install.plist.parent().unwrap()).unwrap();
    std::fs::write(&install.plist, "kept").unwrap();
    std::fs::write(
        &install.settings,
        json!({"env": {
            "ANTHROPIC_BASE_URL": base_url(3337),
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"
        }})
        .to_string(),
    )
    .unwrap();
    set_session_hook(&install.settings, false).unwrap();
    let lctl = FakeLaunchctl::default();
    lctl.loaded.set(true);
    remove_integration_with(&install, &lctl, &Config::default()).unwrap();
    assert!(lctl.calls.borrow().is_empty());
    assert!(install.plist.exists());
    assert_eq!(
        read_json(&install.settings),
        json!({"env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"}})
    );
}
