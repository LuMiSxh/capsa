// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use super::harness::Harness;

fn is_bg_update(m: &Value) -> bool {
    m["params"]["update"]["toolCallId"] == "toolu_bg"
}

fn bg_status(m: &Value) -> Option<&str> {
    m["params"]["update"]["status"].as_str()
}

/// Runs a background-launch scenario until the card reports `completed`.
pub(crate) async fn background_card(h: &mut Harness, prompt: &str) -> Vec<Value> {
    h.prompt(1, prompt).await;
    let mut seen = h
        .recv_until(|m| is_bg_update(m) && bg_status(m) == Some("completed"))
        .await;
    // The announcement follows right after the card's final update.
    seen.push(h.recv().await);
    seen
}

fn assert_finish_entry(seen: &[Value], entry_text: &str) {
    let (entry, before) = seen.split_last().unwrap();
    assert!(
        is_bg_update(before.last().unwrap()),
        "entry follows the card"
    );
    let u = &entry["params"]["update"];
    assert_eq!(u["sessionUpdate"], "tool_call");
    assert_eq!(u["toolCallId"], "pons-finished-toolu_bg");
    assert_eq!(u["title"], "Agent finished");
    assert_eq!(u["kind"], "other");
    assert_eq!(u["status"], "completed");
    assert_eq!(u["content"][0]["content"]["text"], entry_text);
}

fn assert_card_stays_open_until_done(seen: &[Value], lines: &str, entry_text: &str) {
    assert_finish_entry(seen, entry_text);
    let cards: Vec<&Value> = seen.iter().filter(|m| is_bg_update(m)).collect();
    let (last, earlier) = cards.split_last().unwrap();
    assert!(
        earlier.iter().all(|m| bg_status(m) != Some("completed")),
        "no completed status before the end: {cards:?}"
    );
    assert!(
        earlier.iter().any(|m| bg_status(m) == Some("in_progress")),
        "card shown as running: {cards:?}"
    );
    assert!(
        !cards
            .iter()
            .any(|m| m.to_string().contains("Async agent launched")),
        "launch blob never reaches the client"
    );
    let content = last["params"]["update"]["content"].as_array().unwrap();
    let text = content.last().unwrap()["content"]["text"].as_str().unwrap();
    assert_eq!(text, format!("done: Read src/b.rs\n{lines}"));
    let title = last["params"]["update"]["title"].as_str().unwrap();
    assert!(
        title.starts_with("Agent: done, 1 tool call, "),
        "final card title: {title}"
    );
    assert!(
        cards.iter().any(|m| m["params"]["update"]["title"]
            .as_str()
            .is_some_and(|t| t.starts_with("Agent: running "))),
        "running title shown: {cards:?}"
    );
    assert!(
        seen.iter().all(|m| m.to_string().is_ascii()),
        "wire output is ASCII: {seen:?}"
    );
}

#[tokio::test]
async fn background_card_completes_on_task_notification() {
    let mut h = Harness::start(&[]);
    let seen = background_card(&mut h, "background-notify").await;
    assert_card_stays_open_until_done(&seen, "done - 1 tool call", "1 tool call");
}

#[tokio::test]
async fn background_card_completes_after_idle_without_notification() {
    let mut h = Harness::start(&["--subagent-idle-ms", "400"]);
    let seen = background_card(&mut h, "background-text").await;
    assert_card_stays_open_until_done(
        &seen,
        "agent: All clear.\ndone (no activity for 0.4s) - 1 tool call",
        "1 tool call\nAll clear.",
    );
    // The roster went out as a plan while the subagent ran.
    let plan = seen
        .iter()
        .find(|m| m["params"]["update"]["sessionUpdate"] == "plan")
        .expect("roster plan");
    let entry = &plan["params"]["update"]["entries"][0];
    assert_eq!(entry["priority"], "medium");
    // Whether the first plan still says "starting" depends on timing.
    let content = entry["content"].as_str().unwrap();
    assert!(
        content.starts_with("worker - Fix ledger torn tail: ") && content.ends_with("s)"),
        "{entry}"
    );
    assert!(matches!(
        entry["status"].as_str(),
        Some("pending" | "in_progress")
    ));
}

#[tokio::test]
async fn roster_can_be_disabled() {
    let mut h = Harness::start(&["--subagent-idle-ms", "400", "--no-roster"]);
    let seen = background_card(&mut h, "background-text").await;
    assert!(
        seen.iter()
            .all(|m| m["params"]["update"]["sessionUpdate"] != "plan")
    );
}

pub(crate) fn is_finish_entry(m: &Value) -> bool {
    m["params"]["update"]["toolCallId"]
        .as_str()
        .is_some_and(|id| id.starts_with("pons-finished-"))
}

pub(crate) fn is_chunk(m: &Value) -> bool {
    m["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
}

#[tokio::test]
async fn finish_entry_waits_for_a_pause_in_the_main_agents_text() {
    let mut h = Harness::start(&["--subagent-idle-ms", "300", "--entry-gap-ms", "400"]);
    h.prompt(1, "stream").await;
    let seen = h.recv_until(is_finish_entry).await;
    let chunks = seen.iter().filter(|m| is_chunk(m)).count();
    assert_eq!(chunks, 8, "the entry does not split the stream: {seen:?}");
    assert!(
        seen.iter()
            .any(|m| is_bg_update(m) && bg_status(m) == Some("completed")),
        "the card itself finished before the entry"
    );
}

#[tokio::test]
async fn finish_entry_goes_out_before_the_prompt_response() {
    let mut h = Harness::start(&["--subagent-idle-ms", "300", "--entry-gap-ms", "60000"]);
    h.prompt(1, "stream").await;
    h.prompt(2, "finish").await;
    let seen = h.recv_until(|m| m["id"] == 1).await;
    let (response, before) = seen.split_last().unwrap();
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert!(
        before.iter().any(is_finish_entry),
        "entry sent before the response: {seen:?}"
    );
}
