// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use serde_json::Value;
use tokio::time::sleep;

use super::harness::{
    Harness, answers, barrier, initialize_agent, is_cancel_seen, is_stop_mark, send_message,
    start_background_turn, start_with_agent_args,
};

/// Sends a message while a background turn is streaming a tool call's input
/// and returns what reached the client up to the steering request.
async fn steer_while_tool_streams(hold: &str) -> Vec<Value> {
    let mut h = start_with_agent_args(&["--steer-hold-ms", hold], &["--steer", "injected"]);
    initialize_agent(&mut h).await;
    h.prompt(1, "background-tool").await;
    h.recv_until(|m| m["params"]["update"]["toolCallId"] == "m1")
        .await;
    send_message(&mut h, 2, "ping").await;
    h.recv_until(|m| m["method"] == "mock/steer_seen").await
}

fn is_m1_running(m: &Value) -> bool {
    m["params"]["update"]["toolCallId"] == "m1" && m["params"]["update"]["status"] == "in_progress"
}

#[tokio::test]
async fn steering_waits_for_the_streaming_tool_call() {
    let seen = steer_while_tool_streams("1500").await;
    assert!(seen.iter().any(is_m1_running), "{seen:?}");
}

#[tokio::test]
async fn steering_does_not_wait_without_a_hold() {
    let seen = steer_while_tool_streams("0").await;
    assert!(!seen.iter().any(is_m1_running), "{seen:?}");
}

#[tokio::test]
async fn a_message_while_subagents_run_is_steered_and_the_subagents_survive() {
    let mut h = start_with_agent_args(&[], &["--steer", "injected"]);
    let init = initialize_agent(&mut h).await;
    assert_eq!(
        init["result"]["_meta"]["steering"]["supported"], true,
        "{init}"
    );
    start_background_turn(&mut h).await;

    // Zed answers prompt 1 as cancelled at once, then steers the message.
    h.cancel().await;
    let cancelled = h.recv().await;
    assert_eq!(cancelled["id"], 1);
    assert_eq!(cancelled["result"]["stopReason"], "cancelled");
    sleep(Duration::from_millis(100)).await;
    h.prompt(2, "ping").await;
    let seen = h.recv_until(|m| m["method"] == "mock/steer_seen").await;
    let steer = seen.last().unwrap();
    assert_eq!(steer["params"]["sessionId"], "s1");
    assert_eq!(steer["params"]["prompt"][0]["text"], "ping");

    // Past the decision window nothing reached the agent as a cancel and no
    // card was marked stopped or finished.
    sleep(Duration::from_millis(700)).await;
    let seen = barrier(&mut h, 9).await;
    assert!(!seen.iter().any(is_cancel_seen), "{seen:?}");
    assert!(!seen.iter().any(is_stop_mark), "{seen:?}");
    assert_eq!(answers(&seen, 2), 0, "{seen:?}");

    // A second message ends the held turn (the mock does so on `finish`):
    // the client gets exactly one answer for each prompt id: prompt 2 was
    // cancelled by the second message, prompt 3 gets the turn's result.
    send_message(&mut h, 3, "finish").await;
    let mut seen = h.recv_until(|m| m["id"] == 3).await;
    assert_eq!(seen.last().unwrap()["result"]["steered"], true, "{seen:?}");
    seen.extend(barrier(&mut h, 10).await);
    assert!(!seen.iter().any(is_cancel_seen), "{seen:?}");
    assert!(
        !seen
            .iter()
            .any(|m| m["params"]["update"]["status"] == "failed")
    );
    assert_eq!(answers(&seen, 1), 0, "{seen:?}");
    assert_eq!(answers(&seen, 2), 1, "{seen:?}");
    let two = seen.iter().find(|m| m["id"] == 2).unwrap();
    assert_eq!(two["result"]["stopReason"], "cancelled");
    assert_eq!(answers(&seen, 3), 1, "{seen:?}");
}

#[tokio::test]
async fn without_steering_a_message_is_queued_and_the_cancel_never_reaches_the_agent() {
    let mut h = Harness::start(&[]);
    start_background_turn(&mut h).await;
    send_message(&mut h, 2, "finish").await;
    let mut seen = h.recv_until(|m| m["id"] == 2).await;
    // Updates of the first turn may arrive before its answer.
    let first = seen.iter().find(|m| m["id"] == 1).expect("turn 1 answered");
    assert_eq!(first["result"]["stopReason"], "cancelled", "{seen:?}");
    seen.extend(barrier(&mut h, 9).await);
    assert!(!seen.iter().any(is_cancel_seen), "{seen:?}");
    assert_eq!(answers(&seen, 1), 1, "{seen:?}");
    assert_eq!(answers(&seen, 2), 1, "{seen:?}");
}

#[tokio::test]
async fn steering_that_fails_or_is_declined_falls_back_to_queueing() {
    for mode in ["error", "prompt-required"] {
        let mut h = start_with_agent_args(&[], &["--steer", mode]);
        initialize_agent(&mut h).await;
        start_background_turn(&mut h).await;
        send_message(&mut h, 2, "finish").await;
        let mut seen = h.recv_until(|m| m["id"] == 2).await;
        assert!(
            seen.iter()
                .any(|m| m["method"] == "mock/steer_seen" || mode == "error")
        );
        seen.extend(barrier(&mut h, 9).await);
        assert!(!seen.iter().any(is_cancel_seen), "{mode}: {seen:?}");
        assert!(
            !seen
                .iter()
                .any(|m| m["params"]["update"]["status"] == "failed"),
            "{mode}: {seen:?}"
        );
        assert_eq!(answers(&seen, 1), 1, "{mode}: {seen:?}");
        assert_eq!(answers(&seen, 2), 1, "{mode}: {seen:?}");
        assert!(
            !seen.iter().any(|m| m["id"]
                .as_str()
                .is_some_and(|i| i.starts_with("pons-steer"))),
            "{mode}: the steering reply must not reach the client: {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_steered_message_that_started_a_new_turn_is_answered_by_pons() {
    let mut h = start_with_agent_args(&[], &["--steer", "started"]);
    initialize_agent(&mut h).await;
    start_background_turn(&mut h).await;
    send_message(&mut h, 2, "hello").await;
    let seen = h.recv_until(|m| m["id"] == 2).await;
    assert_eq!(seen.last().unwrap()["result"]["stopReason"], "end_turn");
    assert!(!barrier(&mut h, 9).await.iter().any(is_cancel_seen));
}

#[tokio::test]
async fn queue_mode_does_not_steer_even_when_the_agent_can() {
    let mut h = start_with_agent_args(&["--interrupt-mode", "queue"], &["--steer", "injected"]);
    initialize_agent(&mut h).await;
    start_background_turn(&mut h).await;
    send_message(&mut h, 2, "finish").await;
    let mut seen = h.recv_until(|m| m["id"] == 2).await;
    seen.extend(barrier(&mut h, 9).await);
    assert!(
        !seen.iter().any(|m| m["method"] == "mock/steer_seen"),
        "{seen:?}"
    );
    assert!(!seen.iter().any(is_cancel_seen), "{seen:?}");
}
