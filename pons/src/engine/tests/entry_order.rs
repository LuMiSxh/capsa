// SPDX-License-Identifier: MPL-2.0

use super::*;

/// Builds an engine in which `idle_subagent` finishes at t0 + 21s while the main agent
/// streams text.
fn finished_mid_sentence(t0: Instant) -> Engine {
    let mut e = idle_subagent(t0);
    e.on_agent(&chunk("I told this "), t0 + ms(20_950));
    let out = e.tick(t0 + secs(21));
    assert!(client_lines(&out).iter().any(is_card_done));
    assert!(entries(&out).is_empty(), "held mid-sentence");
    e
}

#[test]
fn entry_waits_for_a_pause_in_the_main_agents_text() {
    let t0 = Instant::now();
    let mut e = finished_mid_sentence(t0);
    e.on_agent(&chunk("agent to count"), t0 + ms(21_200));
    assert!(entries(&e.tick(t0 + ms(21_700))).is_empty());
    assert!(entries(&e.tick(t0 + ms(21_799))).is_empty());
    let out = e.tick(t0 + ms(21_800));
    assert_eq!(entries(&out).len(), 1, "600 ms after the last chunk");
    assert!(entries(&e.tick(t0 + ms(30_000))).is_empty(), "sent once");
}

#[test]
fn thought_chunks_also_hold_the_entry() {
    let t0 = Instant::now();
    let mut e = finished_mid_sentence(t0);
    e.on_agent(
        &update(json!({"sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "hmm"}})),
        t0 + ms(21_500),
    );
    assert!(entries(&e.tick(t0 + ms(22_000))).is_empty());
    assert_eq!(entries(&e.tick(t0 + ms(22_100))).len(), 1);
}

#[test]
fn entry_goes_out_before_the_next_main_agent_tool_call() {
    let t0 = Instant::now();
    let mut e = finished_mid_sentence(t0);
    let out = e.on_agent(&main_tool_call(), t0 + ms(21_100));
    let lines = client_lines(&out);
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0]["params"]["update"]["toolCallId"]
            .as_str()
            .unwrap()
            .starts_with("pons-finished-")
    );
    assert_eq!(lines[1]["params"]["update"]["toolCallId"], "m1");
}

#[test]
fn entry_goes_out_before_the_turn_end_response() {
    let t0 = Instant::now();
    let mut e = finished_mid_sentence(t0);
    let out = e.on_agent(&response(1), t0 + ms(21_100));
    let lines = client_lines(&out);
    assert_eq!(lines.len(), 2, "entry, then the response");
    assert_eq!(
        lines[0]["params"]["update"]["toolCallId"],
        "pons-finished-A"
    );
    assert_eq!(lines[1]["id"], 1);
}

#[test]
fn entry_goes_out_before_a_synthesized_cancelled_response() {
    let t0 = Instant::now();
    let mut e = idle_subagent(t0);
    // A second subagent, still active, keeps the cancel filter armed.
    e.on_agent(&real_parent("B"), t0 + secs(2));
    e.on_agent(&real_launch_result("B"), t0 + secs(2));
    e.on_agent(&kid("B", "b1", "Read b"), t0 + secs(15));
    e.on_agent(&chunk("I told this "), t0 + ms(20_950));
    let out = e.tick(t0 + secs(21));
    assert!(entries(&out).is_empty(), "held mid-sentence");
    let out = e.on_client(&cancel(), t0 + secs(24));
    let lines = client_lines(&out);
    assert_eq!(lines.len(), 2);
    assert!(lines[0]["params"]["update"]["toolCallId"].is_string());
    assert_eq!(lines[1]["result"]["stopReason"], "cancelled");
}

#[test]
fn held_entries_keep_their_finish_order() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&child_text("A", "done"), t0 + secs(1));
    e.on_agent(&child_text("B", "done"), t0 + secs(1));
    e.on_agent(&chunk("text"), t0 + secs(21));
    // Both go idle in the same tick; B was launched first by id order.
    let out = e.tick(t0 + ms(21_050));
    assert!(entries(&out).is_empty());
    let out = e.tick(t0 + ms(21_700));
    let ids: Vec<_> = entries(&out)
        .iter()
        .map(|m| m["params"]["update"]["toolCallId"].clone())
        .collect();
    assert_eq!(ids, ["pons-finished-A", "pons-finished-B"]);
}

#[test]
fn held_entry_is_dropped_when_the_card_reopens() {
    let t0 = Instant::now();
    let mut e = finished_mid_sentence(t0);
    let out = e.on_agent(&kid("A", "a2", "Read a2"), t0 + ms(21_100));
    assert_eq!(
        updates_for(&out, "A")[0]["params"]["update"]["status"],
        "in_progress"
    );
    assert!(entries(&e.tick(t0 + secs(25))).is_empty());
    assert!(e.held.is_empty());
}

#[test]
fn zero_gap_sends_the_entry_at_once() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        entry_gap: Duration::ZERO,
        ..test_config()
    });
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&child_text("A", "x"), t0);
    e.on_agent(&chunk("x"), t0 + ms(20_999));
    assert_eq!(entries(&e.tick(t0 + secs(21))).len(), 1);
}

#[test]
fn entry_is_sent_at_once_when_nothing_was_streamed() {
    let t0 = Instant::now();
    let mut e = idle_subagent(t0);
    e.on_agent(&chunk("early"), t0 + secs(2));
    // Long pause since the last chunk: no wait at all.
    assert_eq!(entries(&e.tick(t0 + secs(21))).len(), 1);
}

#[test]
fn marker_ends_the_notification_turn_and_releases_the_entry() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(2));
    e.on_agent(&kid_ended("B", "b1", "Read b"), t0 + secs(5));
    // The notification turn streams, then ends with the marker.
    e.on_agent(&chunk("A is done"), t0 + ms(9_990));
    let out = e.on_agent(&task_notification(), t0 + secs(10));
    let ids: Vec<_> = client_lines(&out)
        .iter()
        .map(|m| m["params"]["update"]["toolCallId"].clone())
        .collect();
    assert_eq!(ids, [json!("A"), json!("pons-finished-A"), Value::Null]);
    assert!(matches!(
        &out[out.len() - 1],
        Action::ToClient {
            synthesized: false,
            ..
        }
    ));
}
