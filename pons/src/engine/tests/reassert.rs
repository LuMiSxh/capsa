// SPDX-License-Identifier: MPL-2.0

use super::*;

/// Two running background subagents A and B under prompt 1. With `steering`,
/// the adapter advertised `_meta.steering` at `initialize`.
fn two_background(t0: Instant, steering: bool) -> Engine {
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    for id in ["A", "B"] {
        e.on_agent(&real_parent(id), t0);
        e.on_agent(&real_launch_result(id), t0 + secs(1));
        e.on_agent(&kid(id, &format!("{id}1"), "Read a"), t0 + secs(2));
    }
    if steering {
        e.on_client(&initialize(false), t0);
        let reply = line(json!({"jsonrpc": "2.0", "id": 0,
            "result": {"_meta": {"steering": {"supported": true}}}}));
        e.on_agent(&reply, t0);
    }
    e
}

/// The card updates of `actions` that assert a running card again.
fn asserted(actions: &[Action]) -> Vec<Value> {
    client_lines(actions)
        .into_iter()
        .filter(|m| {
            m["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && m["params"]["update"]["status"] == "in_progress"
        })
        .collect()
}

fn asserted_ids(actions: &[Action]) -> Vec<String> {
    asserted(actions)
        .iter()
        .map(|m| {
            m["params"]["update"]["toolCallId"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// The sessionUpdate kinds of the client lines, in order.
fn sequence(actions: &[Action]) -> Vec<String> {
    client_lines(actions)
        .iter()
        .map(|m| {
            m["params"]["update"]["sessionUpdate"]
                .as_str()
                .unwrap_or("-")
                .to_owned()
        })
        .collect()
}

/// The status of each entry of the plan sent in `actions`.
fn plan_statuses(actions: &[Action]) -> Vec<String> {
    of_kind(actions, "plan")
        .iter()
        .flat_map(|p| {
            p["params"]["update"]["entries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|e| e["status"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn a_steered_message_asserts_both_running_cards_after_the_cancelled_response() {
    let t0 = Instant::now();
    let mut e = two_background(t0, true);
    let out = e.on_client(&cancel(), t0 + secs(10));
    assert_eq!(client_lines(&out)[0]["result"]["stopReason"], "cancelled");
    let p2 = text_prompt(2, "hello");
    let out = e.on_client(&p2, t0 + secs(10) + ms(108));
    let steer = steer_request(&out).expect("steering sent");
    assert_eq!(steer["params"]["prompt"][0]["text"], "hello");
    assert_eq!(asserted_ids(&out), vec!["A", "B"]);
    for m in asserted(&out) {
        assert!(
            m["params"]["update"]["title"]
                .as_str()
                .is_some_and(|t| t.contains("running"))
        );
    }
    // The steering request goes out before the cards are asserted.
    let steer_at = out
        .iter()
        .position(|a| matches!(a, Action::ToAgent(_)))
        .unwrap();
    let card_at = out
        .iter()
        .position(|a| {
            matches!(
                a,
                Action::ToClient {
                    synthesized: true,
                    ..
                }
            )
        })
        .unwrap();
    assert!(steer_at < card_at, "{out:?}");
    assert_eq!(running(&e), 2);
    // The plan follows the cards, with both running subagents.
    assert_eq!(
        sequence(&out),
        vec!["tool_call_update", "tool_call_update", "plan"]
    );
    assert_eq!(plan_statuses(&out), vec!["in_progress", "in_progress"]);
}

#[test]
fn a_queued_message_asserts_the_running_cards_too() {
    let t0 = Instant::now();
    let mut e = two_background(t0, false);
    e.on_client(&cancel(), t0 + secs(10));
    let p2 = text_prompt(2, "hello");
    let out = e.on_client(&p2, t0 + secs(10) + ms(108));
    assert!(agent_msgs(&out).contains(&serde_json::from_slice(&p2).unwrap()));
    assert_eq!(asserted_ids(&out), vec!["A", "B"]);
    assert_eq!(
        sequence(&out),
        vec!["tool_call_update", "tool_call_update", "plan"]
    );
    assert_eq!(plan_statuses(&out), vec!["in_progress", "in_progress"]);
}

#[test]
fn a_real_stop_asserts_no_card_again() {
    let t0 = Instant::now();
    let mut e = two_background(t0, true);
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.tick(t0 + secs(10) + ms(500));
    assert!(
        agent_msgs(&out)
            .iter()
            .any(|m| m["method"] == "session/cancel")
    );
    assert!(asserted(&out).is_empty(), "{out:?}");
    assert!(failed_or_finished(&out));
    assert_eq!(running(&e), 0);
    // No re-sent plan: whatever plan the stop sends shows no running subagent.
    assert!(
        !plan_statuses(&out).iter().any(|s| s == "in_progress"),
        "{out:?}"
    );
}

#[test]
fn a_finished_card_is_not_asserted_again() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0 + secs(1));
    // Prompt 1's turn ends: its card A is finished.
    e.on_agent(&response(1), t0 + secs(2));
    e.on_client(&prompt(2), t0 + secs(3));
    e.on_agent(&real_parent("B"), t0 + secs(3));
    e.on_agent(&real_launch_result("B"), t0 + secs(4));
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(3, "hello"), t0 + secs(10) + ms(108));
    assert_eq!(asserted_ids(&out), vec!["B"]);
    assert_eq!(sequence(&out), vec!["tool_call_update", "plan"]);
    // B has no tool call yet, so its roster entry is pending; A is completed.
    assert_eq!(plan_statuses(&out), vec!["completed", "pending"]);
}
