// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use tokio::time::sleep;

use super::harness::{Harness, answers, barrier, start_with_agent_args};

#[tokio::test]
async fn a_turn_that_never_ends_is_answered_by_the_watchdog() {
    let mut h = Harness::start(&["--turn-stall-ms", "300"]);
    h.prompt(1, "final-text").await;
    let seen = h.recv_until(|m| m["id"] == 1).await;
    let last = seen.last().unwrap();
    assert_eq!(last["result"]["stopReason"], "end_turn", "{seen:?}");
    // The mock answers prompt 1 for real only when told to. Its late answer
    // is dropped: the client sees exactly one response per prompt.
    h.prompt(2, "finish").await;
    let seen = h.recv_until(|m| m["id"] == 2).await;
    assert_eq!(answers(&seen, 1), 0, "{seen:?}");
    assert_eq!(answers(&seen, 2), 1, "{seen:?}");
}

#[tokio::test]
async fn the_watchdog_can_be_turned_off() {
    let mut h = Harness::start(&["--turn-stall-ms", "0"]);
    h.prompt(1, "final-text").await;
    sleep(Duration::from_millis(800)).await;
    let seen = barrier(&mut h, 9).await;
    assert_eq!(answers(&seen, 1), 0, "{seen:?}");
}

#[tokio::test]
async fn a_stop_fails_the_tool_calls_the_adapter_left_open() {
    let mut h = start_with_agent_args(&[], &["--cancel-ends"]);
    h.prompt(1, "tool-open").await;
    h.recv_until(|m| m["params"]["update"]["toolCallId"] == "m1")
        .await;
    h.cancel().await;
    let seen = h.recv_until(|m| m["id"] == 1).await;
    let failed: Vec<_> = seen
        .iter()
        .filter(|m| m["params"]["update"]["status"] == "failed")
        .collect();
    assert_eq!(failed.len(), 1, "{seen:?}");
    assert_eq!(failed[0]["params"]["update"]["toolCallId"], "m1");
    assert_eq!(
        failed[0]["params"]["update"]["sessionUpdate"],
        "tool_call_update"
    );
    // It comes before the response, which is still the agent's.
    assert_eq!(seen.last().unwrap()["result"]["stopReason"], "cancelled");
    assert_eq!(seen.len(), 3, "{seen:?}");
}

#[tokio::test]
async fn the_ghost_call_closing_can_be_turned_off() {
    let mut h = start_with_agent_args(&["--no-close-ghost-calls"], &["--cancel-ends"]);
    h.prompt(1, "tool-open").await;
    h.recv_until(|m| m["params"]["update"]["toolCallId"] == "m1")
        .await;
    h.cancel().await;
    let seen = h.recv_until(|m| m["id"] == 1).await;
    assert!(
        seen.iter()
            .all(|m| m["params"]["update"]["status"] != "failed"),
        "{seen:?}"
    );
}
