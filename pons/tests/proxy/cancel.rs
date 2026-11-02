// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use tokio::time::sleep;

use super::harness::{
    Harness, answers, barrier, is_cancel_seen, is_stop_mark, start_background_turn, temp_path,
};

#[tokio::test]
async fn a_cancel_without_a_prompt_is_forwarded_after_the_window_and_stops_the_cards() {
    let mut h = Harness::start(&["--cancel-defer-ms", "300"]);
    start_background_turn(&mut h).await;
    h.cancel().await;
    let cancelled = h.recv().await;
    assert_eq!(cancelled["id"], 1);
    assert_eq!(cancelled["result"]["stopReason"], "cancelled");
    let seen = h.recv_until(is_cancel_seen).await;
    let marks: Vec<_> = seen.iter().filter(|m| is_stop_mark(m)).collect();
    assert!(
        marks
            .iter()
            .any(|m| m["params"]["update"]["toolCallId"] == "pons-finished-toolu_bg"),
        "{seen:?}"
    );
    let entries = marks
        .iter()
        .filter(|m| m["params"]["update"]["toolCallId"] == "pons-finished-toolu_bg")
        .count();
    assert_eq!(entries, 1, "{seen:?}");
    // Exactly once: nothing more follows, and the cancel is not repeated.
    let rest = barrier(&mut h, 9).await;
    assert!(
        !rest.iter().any(is_cancel_seen) && !rest.iter().any(is_stop_mark),
        "{rest:?}"
    );
    assert_eq!(answers(&rest, 1), 0);
}

#[tokio::test]
async fn a_second_cancel_inside_the_window_is_forwarded_at_once() {
    let mut h = Harness::start(&["--cancel-defer-ms", "4000"]);
    start_background_turn(&mut h).await;
    h.cancel().await;
    assert_eq!(
        h.recv_until(|m| m.get("result").is_some())
            .await
            .last()
            .unwrap()["result"]["stopReason"],
        "cancelled"
    );
    let t = std::time::Instant::now();
    h.cancel().await;
    h.recv_until(is_cancel_seen).await;
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn cancel_mode_forwards_the_cancel_at_once() {
    let mut h = Harness::start(&["--interrupt-mode", "cancel"]);
    start_background_turn(&mut h).await;
    h.cancel().await;
    // Other updates of the running turn may arrive first; wait for the cancel.
    let seen = h.recv_until(is_cancel_seen).await;
    assert!(seen.last().is_some_and(is_cancel_seen), "{seen:?}");
}

#[tokio::test]
async fn the_deprecated_quiet_ms_flag_is_accepted_and_ignored() {
    let log = temp_path("quiet.log");
    let _ = std::fs::remove_file(&log);
    let mut h = Harness::start(&["--quiet-ms", "1", "--log", log.to_str().unwrap()]);
    start_background_turn(&mut h).await;
    h.cancel().await;
    // Held, not forwarded: the synthesized answer comes back. Updates (roster, title
    // refresh) may arrive before it, so skip to the response.
    let seen = h.recv_until(|m| m.get("result").is_some()).await;
    assert_eq!(seen.last().unwrap()["result"]["stopReason"], "cancelled");
    // Info lines are buffered: pons writes them out when it exits.
    drop(h.stdin);
    tokio::time::timeout(std::time::Duration::from_secs(10), h.child.wait())
        .await
        .expect("pons exits")
        .unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    let _ = std::fs::remove_file(&log);
    assert!(text.contains("--quiet-ms is deprecated"), "{text}");
}

/// Holds a first cancel and drops it by sending a message (answering prompt 1
/// as cancelled).
async fn drop_first_cancel(h: &mut Harness) {
    h.start_subagent_turn().await;
    h.cancel().await;
    assert_eq!(
        h.recv_until(|m| m.get("result").is_some())
            .await
            .last()
            .unwrap()["result"]["stopReason"],
        "cancelled"
    );
}

#[tokio::test]
async fn repeated_cancel_within_stop_window_is_forwarded() {
    let mut h = Harness::start(&["--stop-window-ms", "5000"]);
    drop_first_cancel(&mut h).await;
    sleep(Duration::from_millis(100)).await;
    h.cancel().await;
    assert_eq!(
        h.recv_until(is_cancel_seen).await.last().unwrap()["method"],
        "mock/cancel_seen"
    );
}

#[tokio::test]
async fn a_cancel_in_a_new_prompt_is_not_a_stop_escape() {
    let mut h = Harness::start(&["--stop-window-ms", "5000"]);
    drop_first_cancel(&mut h).await;
    h.prompt(2, "plain").await;
    sleep(Duration::from_millis(300)).await;
    h.cancel().await;
    let cancelled = h.recv().await;
    assert_eq!(cancelled["id"], 2);
    assert_eq!(cancelled["result"]["stopReason"], "cancelled");
}

#[tokio::test]
async fn a_later_cancel_is_held_again() {
    let mut h = Harness::start(&["--stop-window-ms", "300"]);
    drop_first_cancel(&mut h).await;
    h.prompt(2, "plain").await;
    sleep(Duration::from_millis(500)).await;
    h.cancel().await;
    let cancelled = h.recv().await;
    assert_eq!(cancelled["id"], 2);
    assert_eq!(cancelled["result"]["stopReason"], "cancelled");
    h.prompt(3, "finish").await;
    let rest = h.recv_until(|m| m["id"] == 3).await;
    assert!(rest.iter().all(|m| m["method"] != "mock/cancel_seen"));
}

#[tokio::test]
async fn forwards_cancel_without_subagent() {
    let mut h = Harness::start(&[]);
    h.prompt(1, "plain").await;
    h.cancel().await;
    assert_eq!(h.recv().await["method"], "mock/cancel_seen");
}

#[tokio::test]
async fn forwards_cancel_when_filter_disabled() {
    let mut h = Harness::start(&["--no-cancel-filter"]);
    h.start_subagent_turn().await;
    sleep(Duration::from_millis(50)).await;
    h.cancel().await;
    assert_eq!(h.recv().await["method"], "mock/cancel_seen");
}
