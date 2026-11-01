// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use serde_json::Value;
use tokio::process::Command;

use super::harness::{Harness, mock_agent, pons_command, temp_path};

/// The rate-limit event as claude-agent-acp forwards it; the mock agent prints this same line.
const RATE_LIMIT_LINE: &str = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"usage_update","used":1,"size":2,"_meta":{"_claude/rateLimit":{"status":"allowed","resetsAt":1793530800,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.45,"resetsAt":1793530800},"seven_day":{"utilization":0.05,"resetsAt":1794106800}}}}}}}"#;

fn census_command(state: &Path, args: &[&str]) -> Command {
    let mut command = pons_command(args);
    command
        .arg("--")
        .arg(mock_agent())
        .env("XDG_STATE_HOME", state);
    command
}

#[tokio::test]
async fn a_rate_limit_line_is_forwarded_unchanged_and_each_window_is_recorded_once() {
    let state = temp_path("census-state");
    let _ = std::fs::remove_dir_all(&state);
    let mut h = Harness::spawn(census_command(&state, &[]));
    h.prompt(1, "rate-limit").await;
    h.prompt(2, "rate-limit").await;
    h.prompt(3, "finish").await;
    assert_eq!(h.recv_line().await, RATE_LIMIT_LINE);
    assert_eq!(h.recv_line().await, RATE_LIMIT_LINE);
    h.recv_until(|m| m["id"] == 3).await;

    // The proxy writes snapshots off the forwarding path, so give the writes a moment to land.
    let file = state.join("census").join("snapshots");
    let mut text = String::new();
    for _ in 0..200 {
        text = std::fs::read_to_string(&file).unwrap_or_default();
        if text.lines().count() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // Let a wrongly repeated write land too before counting.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    text = std::fs::read_to_string(&file).unwrap_or(text);
    let mut rows: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        rows.len(),
        2,
        "the repeated value is not recorded again: {text}"
    );
    // The writes are off the forwarding path, so the two windows may land in either order.
    rows.sort_by(|a, b| a["window"].as_str().cmp(&b["window"].as_str()));
    assert_eq!(rows[0]["source"], "pons");
    assert_eq!(rows[0]["window"], "five_hour");
    assert_eq!(rows[0]["used_percentage"], 45.0);
    assert_eq!(rows[0]["resets_at"], 1_793_530_800);
    assert_eq!(rows[1]["window"], "seven_day");
    assert_eq!(rows[1]["used_percentage"], 5.0);
    assert_eq!(rows[1]["resets_at"], 1_794_106_800);
}

#[tokio::test]
async fn no_census_flag_forwards_the_line_and_writes_nothing() {
    let state = temp_path("census-off");
    let _ = std::fs::remove_dir_all(&state);
    let mut h = Harness::spawn(census_command(&state, &["--no-census"]));
    assert_forwarded_without_census(&mut h, &state).await;
}

#[tokio::test]
async fn no_census_env_forwards_the_line_and_writes_nothing() {
    let state = temp_path("census-env-off");
    let _ = std::fs::remove_dir_all(&state);
    let mut command = census_command(&state, &[]);
    command.env("PONS_NO_CENSUS", "1");
    let mut h = Harness::spawn(command);
    assert_forwarded_without_census(&mut h, &state).await;
}

async fn assert_forwarded_without_census(h: &mut Harness, state: &Path) {
    h.prompt(1, "rate-limit").await;
    h.prompt(2, "finish").await;
    assert_eq!(h.recv_line().await, RATE_LIMIT_LINE);
    h.recv_until(|m| m["id"] == 2).await;
    assert!(!state.exists());
}
