// SPDX-License-Identifier: MPL-2.0

use serde_json::json;

use super::harness::{Harness, has_flat_child_update, is_parent_update, temp_path};

async fn send_boom(h: &mut Harness, id: i64) {
    h.send(json!({"jsonrpc": "2.0", "id": id, "method": "echo/boom",
        "params": {"sessionId": "s1"}}))
        .await;
    // The line reached the agent unchanged: its echo comes back.
    let reply = h.recv().await;
    assert_eq!(reply["id"], id);
    assert_eq!(reply["result"]["echoed"], json!({"sessionId": "s1"}));
}

#[tokio::test]
async fn a_failing_rule_forwards_the_line_and_the_session_continues() {
    let mut h = Harness::start(&["--inject-failure", "boom", "--max-failures", "3"]);
    send_boom(&mut h, 1).await;
    send_boom(&mut h, 2).await;
    // Two failures are below the limit: folding still works.
    let seen = h.start_subagent_turn().await;
    assert!(!seen.iter().any(has_flat_child_update), "{seen:?}");
}

#[tokio::test]
async fn repeated_failures_switch_the_session_to_passthrough() {
    let log = temp_path("failsafe.log");
    let _ = std::fs::remove_file(&log);
    let mut h = Harness::start(&[
        "--inject-failure",
        "boom",
        "--max-failures",
        "3",
        "--log",
        log.to_str().unwrap(),
    ]);
    for id in 1..=3 {
        send_boom(&mut h, id).await;
    }
    h.prompt(4, "subagent").await;
    let seen = h.recv_until(has_flat_child_update).await;
    assert!(seen.iter().any(is_parent_update), "{seen:?}");
    // The session keeps working in passthrough.
    h.send(json!({"jsonrpc": "2.0", "id": 5, "method": "echo/ok",
        "params": {"sessionId": "s1"}}))
        .await;
    let reply = h
        .recv_until(|m| m["id"] == 5 && m.get("result").is_some())
        .await;
    assert_eq!(reply.last().unwrap()["result"]["echoed"]["sessionId"], "s1");
    let text = std::fs::read_to_string(&log).unwrap();
    let _ = std::fs::remove_file(&log);
    assert_eq!(
        text.matches("pons fail-safe: rules failed").count(),
        3,
        "{text}"
    );
    assert!(text.contains("switching to pure passthrough"), "{text}");
}
