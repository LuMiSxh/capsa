// SPDX-License-Identifier: MPL-2.0

use serde_json::{Value, json};

use super::harness::{Harness, temp_path};

pub(crate) async fn initialize(h: &mut Harness, id: i64, caps: Value) -> Value {
    h.send(json!({"jsonrpc": "2.0", "id": id, "method": "initialize",
        "params": {"protocolVersion": 1, "clientCapabilities": caps}}))
        .await;
    h.recv().await["result"]["echoed"]["clientCapabilities"].clone()
}

#[tokio::test]
async fn initialize_asks_the_adapter_for_subagent_text() {
    let log = temp_path("transcript.log");
    let _ = std::fs::remove_file(&log);
    let mut h = Harness::start(&["--log", log.to_str().unwrap()]);
    // Zed's shape: no _meta at all.
    let caps = initialize(&mut h, 1, json!({"fs": {"readTextFile": true}})).await;
    assert_eq!(
        caps,
        json!({"fs": {"readTextFile": true}, "_meta": {"subagent-transcript": true}})
    );
    // Info lines are buffered: pons writes them out when it exits.
    drop(h.stdin);
    tokio::time::timeout(std::time::Duration::from_secs(10), h.child.wait())
        .await
        .expect("pons exits")
        .unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    let _ = std::fs::remove_file(&log);
    assert_eq!(
        text.matches("asked the adapter to forward subagent text")
            .count(),
        1,
        "{text}"
    );
}

#[tokio::test]
async fn initialize_is_untouched_with_the_flag_native_subagents_and_the_failsafe() {
    let caps = json!({"fs": {}});
    let mut h = Harness::start(&["--no-subagent-text"]);
    assert_eq!(initialize(&mut h, 1, caps.clone()).await, caps);
    let mut h = Harness::start(&[]);
    let native = json!({"subagents": {}});
    assert_eq!(initialize(&mut h, 1, native.clone()).await, native);
    // A failing rule forwards the original line.
    let mut h = Harness::start(&["--inject-failure", "initialize"]);
    assert_eq!(initialize(&mut h, 1, caps.clone()).await, caps);
}
