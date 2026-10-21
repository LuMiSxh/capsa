// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn steering_waits_for_a_streaming_tool_call_and_goes_out_when_it_settles() {
    let t0 = Instant::now();
    for settle in [
        tool_status("m1", "in_progress"),
        tool_status("m1", "completed"),
        update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "m1",
            "rawInput": {"file_path": "a"}, "content": []}),
        ),
    ] {
        let mut e = streaming_tool(t0, test_config());
        e.on_client(&cancel(), t0 + secs(10));
        let out = e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(100));
        assert!(agent_msgs(&out).is_empty(), "{out:?}");
        assert!(has_note(&out, "holding the steering"));
        // A partial refinement keeps it waiting.
        let out = e.on_agent(
            &update(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "m1",
                "rawInput": {"file_path": "a"}}),
            ),
            t0 + secs(10) + ms(300),
        );
        assert!(steer_request(&out).is_none());
        let out = e.on_agent(&settle, t0 + secs(10) + ms(500));
        let req = steer_request(&out).expect("steering sent");
        assert_eq!(req["params"]["prompt"][0]["text"], "hello");
        assert!(has_note(&out, "hold over"));
        assert!(e.held_steers.is_empty());
        // The usual steering flow goes on from here.
        let id = req["id"].as_str().unwrap().to_owned();
        let out = e.on_agent(
            &steer_reply(&id, json!({"outcome": "injected"})),
            t0 + secs(11),
        );
        assert!(has_note(&out, "steering result: injected"));
    }
}

#[test]
fn steering_is_sent_anyway_when_the_hold_runs_out() {
    let t0 = Instant::now();
    let mut e = streaming_tool(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    let sent = t0 + secs(10) + ms(100);
    e.on_client(&text_prompt(2, "hello"), sent);
    assert!(steer_request(&e.tick(sent + ms(1499))).is_none());
    let out = e.tick(sent + ms(1500));
    assert!(steer_request(&out).is_some(), "{out:?}");
    assert!(has_note(&out, "hold time passed"));
    // The held cancel is not decided during the hold.
    assert!(
        !agent_msgs(&out)
            .iter()
            .any(|m| m["method"] == "session/cancel")
    );
}

#[test]
fn a_zero_hold_or_a_settled_tool_steers_at_once() {
    let t0 = Instant::now();
    let cfg = Config {
        steer_hold: Duration::ZERO,
        ..test_config()
    };
    let mut e = streaming_tool(t0, cfg);
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(100));
    assert!(steer_request(&out).is_some());

    let mut e = streaming_tool(t0, test_config());
    e.on_agent(&tool_status("m1", "in_progress"), t0 + secs(9) + ms(500));
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(100));
    assert!(steer_request(&out).is_some());
}

#[test]
fn a_held_steering_is_queued_when_the_old_turn_ends_and_flushed_by_a_cancel() {
    let t0 = Instant::now();
    let mut e = streaming_tool(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    let p2 = text_prompt(2, "hello");
    e.on_client(&p2, t0 + secs(10) + ms(100));
    let out = e.on_agent(&response(1), t0 + secs(10) + ms(300));
    assert_eq!(
        out.iter()
            .filter(|a| **a == Action::ToAgent(p2.clone()))
            .count(),
        1,
        "{out:?}"
    );
    assert!(e.held_steers.is_empty());
    assert!(
        e.tick(t0 + secs(20))
            .iter()
            .all(|a| !matches!(a, Action::ToAgent(_)))
    );

    let mut e = streaming_tool(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(100));
    let out = e.on_client(&cancel(), t0 + secs(10) + ms(400));
    assert!(steer_request(&out).is_some(), "{out:?}");
    assert!(e.held_steers.is_empty());
}

#[test]
fn a_steering_error_about_an_incomplete_tool_call_falls_back_to_the_queue() {
    let t0 = Instant::now();
    let mut e = steering_engine(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    let p2 = text_prompt(2, "hello");
    let out = e.on_client(&p2, t0 + secs(10) + ms(100));
    let id = agent_msgs(&out)[0]["id"].as_str().unwrap().to_owned();
    let reply = line(json!({"jsonrpc": "2.0", "id": id,
        "error": {"code": -32603, "message": "invalid_request: incomplete_tool_call"}}));
    let out = e.on_agent(&reply, t0 + secs(11));
    assert!(client_lines(&out).is_empty());
    assert!(out.contains(&Action::ToAgent(p2)));
    assert!(has_note(&out, "incomplete_tool_call"));
    assert!(has_note(&out, "queued"));
}
