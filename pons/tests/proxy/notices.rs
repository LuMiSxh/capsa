// SPDX-License-Identifier: MPL-2.0

use serde_json::{Value, json};

use super::background::is_finish_entry;
use super::harness::Harness;

async fn notice_scenario(args: &[&str], initialize: Option<Value>) -> Vec<Value> {
    let mut h = Harness::start(args);
    if let Some(init) = initialize {
        h.send(init).await;
    }
    h.prompt(1, "background-notify").await;
    h.recv_until(is_finish_entry).await
}

fn is_notice(m: &Value) -> bool {
    m["params"]["update"]["sessionUpdate"] == "notice"
}

#[tokio::test]
async fn notice_is_sent_when_the_client_supports_it() {
    let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": 1,
            "clientCapabilities": {"session": {"notices": {}}}}});
    let seen = notice_scenario(&[], Some(init)).await;
    let notice = seen.iter().find(|m| is_notice(m)).expect("notice");
    assert_eq!(
        notice["params"]["update"],
        json!({"sessionUpdate": "notice", "severity": "info",
            "title": "Agent finished", "description": "1 tool call, 0s"})
    );
}

#[tokio::test]
async fn notice_is_not_sent_by_default() {
    let seen = notice_scenario(&[], None).await;
    assert!(!seen.iter().any(is_notice));
    let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": 1, "clientCapabilities": {}}});
    let seen = notice_scenario(&[], Some(init)).await;
    assert!(!seen.iter().any(is_notice));
}

#[tokio::test]
async fn notices_flag_forces_the_notice() {
    let seen = notice_scenario(&["--notices"], None).await;
    assert!(seen.iter().any(is_notice));
}

#[tokio::test]
async fn no_notice_flag_suppresses_it() {
    let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": 1,
            "clientCapabilities": {"session": {"notices": {}}}}});
    let seen = notice_scenario(&["--no-notice"], Some(init)).await;
    assert!(!seen.iter().any(is_notice));
}
