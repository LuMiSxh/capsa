// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use serde_json::json;
use tokio::time::sleep;

use super::harness::{Harness, temp_path};

#[tokio::test]
async fn pons_keeps_reading_the_agent_while_the_client_does_not_read() {
    let flag = temp_path("flood.done");
    let _ = std::fs::remove_file(&flag);
    let mut h = Harness::start(&[]);
    h.prompt(1, &format!("flood:{}", flag.display())).await;
    // 3 MiB is far more than the pipes hold; the client reads none of it.
    let mut waited = 0;
    while !flag.exists() {
        assert!(waited < 100, "the agent was stalled by an unread client");
        sleep(Duration::from_millis(50)).await;
        waited += 1;
    }
    let _ = std::fs::remove_file(&flag);
    let seen = h.recv_until(|m| m["id"] == 1).await;
    assert_eq!(seen.len(), 193, "nothing was lost");
}

#[tokio::test]
async fn passes_requests_and_agent_requests_through() {
    let mut h = Harness::start(&[]);
    h.send(json!({"jsonrpc": "2.0", "id": 7, "method": "echo/anything", "params": {"x": [1, 2]}}))
        .await;
    let reply = h.recv().await;
    assert_eq!(reply["id"], 7);
    assert_eq!(reply["result"]["echoed"], json!({"x": [1, 2]}));

    // Agent->client request and the client's response travel unchanged.
    h.send(json!({"jsonrpc": "2.0", "id": 8, "method": "ask", "params": {}}))
        .await;
    let req = h.recv().await;
    assert_eq!(req["id"], "perm-1");
    assert_eq!(req["method"], "session/request_permission");
    h.send(json!({"jsonrpc": "2.0", "id": "perm-1", "result": {"outcome": "allow"}}))
        .await;
    let reply = h.recv().await;
    assert_eq!(reply["id"], 8);
    assert_eq!(reply["result"]["permission"], json!({"outcome": "allow"}));
}

#[tokio::test]
async fn malformed_lines_do_not_kill_the_proxy() {
    let mut h = Harness::start(&[]);
    h.send_raw("this is not json").await;
    h.send(json!({"jsonrpc": "2.0", "id": 1, "method": "echo/ok", "params": {}}))
        .await;
    assert_eq!(h.recv().await["id"], 1);
}
