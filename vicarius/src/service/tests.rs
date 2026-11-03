// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use capsa_core::claude_settings::{Removal, Step};
use serde_json::{Value, json};
use vicarius::config::Config;

use super::{
    BASE_URL, MAX_CONTEXT, check_env_free, is_loaded, plist_is_current, remove_env_keys,
    set_env_keys,
};
use super::{Launchctl, remove_session_hook, set_session_hook, write_plist};
use crate::test_support::{FakeLaunchctl, install_in};

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn write_plist_writes_once_and_leaves_the_current_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    assert!(write_plist(&install).unwrap());
    assert!(plist_is_current(&install));
    assert!(!write_plist(&install).unwrap());
    assert_eq!(
        std::fs::read_to_string(&install.plist).unwrap(),
        install.plist_text()
    );
    assert!(install.log.parent().unwrap().is_dir());
}

#[test]
fn write_plist_replaces_a_stale_file_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    std::fs::create_dir_all(install.plist.parent().unwrap()).unwrap();
    std::fs::write(&install.plist, "stale").unwrap();
    assert!(!plist_is_current(&install));
    assert!(write_plist(&install).unwrap());
    assert!(plist_is_current(&install));
    assert!(!install.plist.with_extension("plist.tmp").exists());
}

#[test]
fn set_env_keys_writes_both_keys_once_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = Config::default();
    let first = set_env_keys(&install.settings, &config).unwrap();
    assert_eq!(
        first,
        [(BASE_URL, Step::Install), (MAX_CONTEXT, Step::Install)]
    );
    let settings = read_json(&install.settings);
    assert_eq!(
        settings["env"]["ANTHROPIC_BASE_URL"],
        "http://127.0.0.1:3337"
    );
    assert_eq!(settings["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], "400000");
    let second = set_env_keys(&install.settings, &config).unwrap();
    assert_eq!(second, [(BASE_URL, Step::Set), (MAX_CONTEXT, Step::Set)]);
}

#[test]
fn remove_env_keys_removes_only_the_values_the_service_set() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let settings = json!({
        "env": {
            "ANTHROPIC_BASE_URL": "http://127.0.0.1:3337",
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"
        }
    });
    std::fs::write(&install.settings, settings.to_string()).unwrap();
    let steps = remove_env_keys(&install.settings, &Config::default()).unwrap();
    assert_eq!(
        steps,
        [(BASE_URL, Removal::Removed), (MAX_CONTEXT, Removal::Kept)]
    );
    assert_eq!(
        read_json(&install.settings),
        json!({"env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"}})
    );
}

#[test]
fn session_hook_is_installed_once_and_a_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    assert_eq!(
        set_session_hook(&install.settings, true).unwrap(),
        Step::Install
    );
    assert!(!install.settings.exists());
    assert_eq!(
        set_session_hook(&install.settings, false).unwrap(),
        Step::Install
    );
    assert_eq!(
        set_session_hook(&install.settings, false).unwrap(),
        Step::Set
    );
    let hooks = read_json(&install.settings);
    assert_eq!(
        hooks["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "vicarius hook session-start"
    );
}

#[test]
fn remove_session_hook_removes_the_hook_and_reports_when_it_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    assert!(!remove_session_hook(&install.settings).unwrap());
    set_session_hook(&install.settings, false).unwrap();
    assert!(remove_session_hook(&install.settings).unwrap());
    assert_eq!(read_json(&install.settings), json!({}));
}

#[test]
fn the_agent_is_loaded_only_after_bootstrap() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let lctl = FakeLaunchctl::default();
    assert!(!is_loaded(&lctl, &install));
    lctl.run(&["bootstrap", "gui/501", "x.plist"]).unwrap();
    assert!(is_loaded(&lctl, &install));
}

#[test]
fn a_hand_set_env_key_that_differs_from_the_config_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_in(dir.path());
    let config = Config::default();
    check_env_free(&install.settings, &config).unwrap();
    std::fs::write(
        &install.settings,
        json!({"env": {
            "ANTHROPIC_BASE_URL": "http://127.0.0.1:3337",
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "400000"
        }})
        .to_string(),
    )
    .unwrap();
    check_env_free(&install.settings, &config).unwrap();
    std::fs::write(
        &install.settings,
        json!({"env": {"ANTHROPIC_BASE_URL": "https://gateway.example"}}).to_string(),
    )
    .unwrap();
    let err = check_env_free(&install.settings, &config).unwrap_err();
    assert!(
        format!("{err:#}").contains("ANTHROPIC_BASE_URL is set to"),
        "{err:#}"
    );
    assert!(capsa_core::hint::find_hint(&err).is_some_and(|hint| hint.contains("settings.json")));
    std::fs::write(
        &install.settings,
        json!({"env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "999"}}).to_string(),
    )
    .unwrap();
    let err = check_env_free(&install.settings, &config).unwrap_err();
    assert!(
        format!("{err:#}").contains("CLAUDE_CODE_MAX_CONTEXT_TOKENS is set to"),
        "{err:#}"
    );
}
