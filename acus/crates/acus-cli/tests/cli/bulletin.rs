// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A Bash call the guard refuses (`sed -i` has no acus replacement).
const BLOCKED: &str = "sed -i '' 's/a/b/' config.json";

fn fresh_home(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("bulletin")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs one hook event with the state and config under `home`, and returns its stdout.
fn hook(home: &Path, input: &serde_json::Value) -> String {
    let mut child = bin()
        .arg("hook")
        .env("XDG_STATE_HOME", home.join("state"))
        .env("ACUS_CONFIG", home.join("acus.toml"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    write!(child.stdin.take().unwrap(), "{input}").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    String::from_utf8(out.stdout).unwrap()
}

fn bash(session: &str, command: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session,
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command},
    })
}

fn session_start(session: &str, source: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session,
        "hook_event_name": "SessionStart",
        "source": source,
    })
}

fn bulletin(home: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(home.join("state/capsa/bulletin/acus.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn block_counts(home: &Path) -> serde_json::Map<String, serde_json::Value> {
    bulletin(home).unwrap()["guard_blocks"]["value"]
        .as_object()
        .unwrap()
        .clone()
}

#[test]
fn a_block_increments_the_session_count_in_the_bulletin() {
    let home = fresh_home("block");
    for _ in 0..2 {
        let out = hook(&home, &bash("s1", BLOCKED));
        assert!(out.contains(r#""permissionDecision":"deny""#), "{out}");
    }
    let value = bulletin(&home).unwrap();
    assert_eq!(value["guard_blocks"]["value"]["s1"], 2);
    assert_eq!(value["guard_blocks"]["ttl"], 86_400);
}

#[test]
fn session_start_registers_the_session_and_keeps_its_count_after_compaction() {
    let home = fresh_home("register");
    hook(&home, &session_start("s2", "startup"));
    assert_eq!(block_counts(&home)["s2"], 0);
    hook(&home, &bash("s2", BLOCKED));
    hook(&home, &session_start("s2", "compact"));
    assert_eq!(block_counts(&home)["s2"], 1);
}

#[test]
fn the_bulletin_keeps_the_16_latest_sessions() {
    let home = fresh_home("sixteen");
    for i in 0..17 {
        hook(&home, &session_start(&format!("s{i:02}"), "startup"));
    }
    let counts = block_counts(&home);
    assert_eq!(counts.len(), 16);
    assert!(counts.contains_key("s16"));
    assert!(
        !counts.contains_key("s00"),
        "the oldest session is dropped first"
    );
}

#[test]
fn an_allowed_call_does_not_write_the_bulletin() {
    let home = fresh_home("allow");
    let out = hook(&home, &bash("s3", "ls"));
    assert_eq!(out, "");
    assert!(bulletin(&home).is_none());
}
