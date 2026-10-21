// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_message_without_advertised_steering_is_queued_and_the_cards_survive() {
    // Real timing from the log: cancel, then the prompt 108 ms later.
    let t0 = Instant::now();
    let mut e = five_background(t0, test_config());
    let out = e.on_client(&cancel(), t0 + secs(10));
    assert_eq!(client_lines(&out)[0]["result"]["stopReason"], "cancelled");
    let p2 = text_prompt(2, "hello");
    let out = e.on_client(&p2, t0 + secs(10) + ms(108));
    assert_eq!(
        out.iter()
            .filter(|a| **a == Action::ToAgent(p2.clone()))
            .count(),
        1
    );
    assert!(
        agent_msgs(&out)
            .iter()
            .all(|m| m["method"] != "session/cancel")
    );
    assert!(has_note(&out, "message sent while subagents run"));
    assert!(has_note(&out, "queued"));
    // The window passes with nothing sent: the cancel is gone for good.
    let out = e.tick(t0 + secs(11));
    assert!(agent_msgs(&out).is_empty() && !failed_or_finished(&out));
    // The adapter answers prompt 1 (swallowed) once prompt 2 took over.
    let out = e.on_agent(&response(1), t0 + secs(12));
    assert!(client_lines(&out).is_empty());
    assert!(!failed_or_finished(&out), "{out:?}");
    assert!(has_note(&out, "state is left alone"));
    assert_eq!(running(&e), 5);
    // Prompt 2's own response ends the turn once, and only then.
    let out = e.on_agent(&response(2), t0 + secs(20));
    assert_eq!(
        client_lines(&out).iter().filter(|m| m["id"] == 2).count(),
        1
    );
}

#[test]
fn queue_mode_queues_even_when_steering_is_advertised() {
    let t0 = Instant::now();
    let mut e = steering_engine(
        t0,
        Config {
            interrupt: Some(InterruptMode::Queue),
            ..test_config()
        },
    );
    e.on_client(&cancel(), t0 + secs(10));
    let p2 = text_prompt(2, "hi");
    let out = e.on_client(&p2, t0 + secs(10) + ms(108));
    assert_eq!(agent_msgs(&out).len(), 1);
    assert_eq!(agent_msgs(&out)[0]["method"], "session/prompt");
    assert!(has_note(&out, "--interrupt-mode queue"));
}

#[test]
fn a_message_is_steered_into_the_running_turn() {
    let t0 = Instant::now();
    let mut e = steering_engine(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(108));
    let sent = agent_msgs(&out);
    assert_eq!(sent.len(), 1, "{out:?}");
    assert_eq!(sent[0]["method"], "_session/steering");
    assert_eq!(sent[0]["params"]["sessionId"], "s");
    assert_eq!(sent[0]["params"]["prompt"][0]["text"], "hello");
    assert_eq!(
        sent[0]["params"]["_meta"]["steering"]["idleBehavior"],
        "promptRequired"
    );
    let req = sent[0]["id"].as_str().unwrap().to_owned();
    assert!(req.starts_with("pons-steer-"));
    assert!(has_note(&out, "steered"));

    // Injected: the reply is not shown to the client.
    let out = e.on_agent(
        &steer_reply(&req, json!({"outcome": "injected"})),
        t0 + secs(11),
    );
    assert!(client_lines(&out).is_empty() && agent_msgs(&out).is_empty());
    assert!(has_note(&out, "steering result: injected"));
    // The window passes: no cancel reaches the agent.
    assert!(agent_msgs(&e.tick(t0 + secs(12))).is_empty());
    assert_eq!(running(&e), 5);
    // Updates keep flowing and the cards stay running.
    let out = e.on_agent(&kid("A", "A2", "Read b"), t0 + secs(13));
    assert_eq!(
        updates_for(&out, "A")[0]["params"]["update"]["status"],
        "in_progress"
    );

    // The held turn ends under the old id: the client gets prompt 2's
    // answer, with the result of the old one, exactly once.
    let ended =
        line(json!({"jsonrpc": "2.0", "id": 1, "result": {"stopReason": "end_turn", "usage": 7}}));
    let out = e.on_agent(&ended, t0 + secs(30));
    let answers: Vec<_> = client_lines(&out)
        .into_iter()
        .filter(|m| m.get("result").is_some())
        .collect();
    assert_eq!(answers.len(), 1, "{out:?}");
    assert_eq!(answers[0]["id"], 2);
    assert_eq!(answers[0]["result"]["usage"], 7);
    assert!(e.prompts.is_empty() && e.swallow.is_empty() && e.handoffs.is_empty());
    assert_eq!(e.sessions["s"].prompt_id, None);
}

#[test]
fn the_steered_turns_cards_finish_with_the_new_prompts_response() {
    let t0 = Instant::now();
    let mut e = steering_engine(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(108));
    let req = agent_msgs(&out)[0]["id"].as_str().unwrap().to_owned();
    e.on_agent(
        &steer_reply(&req, json!({"outcome": "injected"})),
        t0 + secs(11),
    );
    let out = e.on_agent(&response(1), t0 + secs(30));
    assert!(
        has_note(&out, "finishing 5 background subagent(s)"),
        "{out:?}"
    );
    assert_eq!(running(&e), 0);
}

#[test]
fn a_stop_after_a_steered_message_cancels_and_answers_nothing_twice() {
    let t0 = Instant::now();
    let mut e = steering_engine(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(108));
    let req = agent_msgs(&out)[0]["id"].as_str().unwrap().to_owned();
    e.on_agent(
        &steer_reply(&req, json!({"outcome": "injected"})),
        t0 + secs(11),
    );
    // Stop: prompt 2 is answered as cancelled, then the cancel goes out.
    let out = e.on_client(&cancel(), t0 + secs(20));
    assert_eq!(client_lines(&out)[0]["id"], 2);
    let out = e.tick(t0 + secs(21));
    assert_eq!(agent_msgs(&out)[0]["method"], "session/cancel");
    // The adapter ends the old prompt: nothing more reaches the client.
    let ended = line(json!({"jsonrpc": "2.0", "id": 1, "result": {"stopReason": "cancelled"}}));
    let out = e.on_agent(&ended, t0 + secs(22));
    assert!(client_lines(&out).is_empty(), "{out:?}");
    assert!(e.prompts.is_empty() && e.swallow.is_empty() && e.handoffs.is_empty());
}

#[test]
fn two_messages_in_a_row_chain_their_handoffs() {
    let t0 = Instant::now();
    let mut e = steering_engine(t0, test_config());
    for (n, at) in [(2, 10), (3, 20)] {
        e.on_client(&cancel(), t0 + secs(at));
        let out = e.on_client(&text_prompt(n, "again"), t0 + secs(at) + ms(100));
        let req = agent_msgs(&out)[0]["id"].as_str().unwrap().to_owned();
        e.on_agent(
            &steer_reply(&req, json!({"outcome": "injected"})),
            t0 + secs(at + 1),
        );
    }
    let out = e.on_agent(&response(1), t0 + secs(40));
    let answers: Vec<_> = client_lines(&out)
        .into_iter()
        .filter(|m| m.get("result").is_some())
        .collect();
    assert_eq!(answers.len(), 1, "{out:?}");
    assert_eq!(answers[0]["id"], 3);
    assert!(e.prompts.is_empty() && e.swallow.is_empty() && e.handoffs.is_empty());
}

#[test]
fn steering_that_does_not_take_falls_back_to_queueing() {
    for reply in [
        json!({"outcome": "promptRequired", "reason": "noRunningTurn"}),
        json!({"error": {"code": -32601, "message": "Method not found"}}),
        json!({"outcome": "surprise"}),
    ] {
        let t0 = Instant::now();
        let mut e = steering_engine(t0, test_config());
        e.on_client(&cancel(), t0 + secs(10));
        let p2 = text_prompt(2, "hello");
        let out = e.on_client(&p2, t0 + secs(10) + ms(108));
        let req = agent_msgs(&out)[0]["id"].as_str().unwrap().to_owned();
        let mut raw = json!({"jsonrpc": "2.0", "id": req});
        raw.as_object_mut()
            .unwrap()
            .extend(
                reply
                    .as_object()
                    .unwrap()
                    .clone()
                    .into_iter()
                    .map(|(k, v)| {
                        if k == "outcome" || k == "reason" {
                            (
                                "result".to_owned(),
                                json!({"outcome": reply["outcome"], "reason": reply["reason"]}),
                            )
                        } else {
                            (k, v)
                        }
                    }),
            );
        let out = e.on_agent(&line(raw), t0 + secs(11));
        assert!(client_lines(&out).is_empty());
        assert_eq!(
            out.iter()
                .filter(|a| **a == Action::ToAgent(p2.clone()))
                .count(),
            1,
            "{out:?}"
        );
        assert!(has_note(&out, "queued"));
        assert!(e.handoffs.is_empty());
        assert_eq!(running(&e), 5);
    }
}

#[test]
fn a_steered_message_that_started_a_new_turn_is_answered_not_duplicated() {
    let t0 = Instant::now();
    let mut e = steering_engine(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(108));
    let req = agent_msgs(&out)[0]["id"].as_str().unwrap().to_owned();
    let out = e.on_agent(
        &steer_reply(&req, json!({"outcome": "startedNewTurn"})),
        t0 + secs(11),
    );
    assert!(agent_msgs(&out).is_empty());
    let answers: Vec<_> = client_lines(&out)
        .into_iter()
        .filter(|m| m.get("result").is_some())
        .collect();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["id"], 2);
}

#[test]
fn the_old_turn_ending_before_the_steering_reply_is_not_lost() {
    let t0 = Instant::now();
    let mut e = steering_engine(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(108));
    let req = agent_msgs(&out)[0]["id"].as_str().unwrap().to_owned();
    let out = e.on_agent(&response(1), t0 + secs(11));
    assert!(client_lines(&out).is_empty());
    assert!(has_note(&out, "before the steering reply"));
    let out = e.on_agent(
        &steer_reply(&req, json!({"outcome": "injected"})),
        t0 + secs(11),
    );
    let answers: Vec<_> = client_lines(&out)
        .into_iter()
        .filter(|m| m.get("result").is_some())
        .collect();
    assert_eq!(answers.len(), 1, "{out:?}");
    assert_eq!(answers[0]["id"], 2);
}

#[test]
fn the_turn_ending_inside_the_window_drops_the_held_cancel() {
    let t0 = Instant::now();
    let mut e = five_background(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    e.on_agent(&response(1), t0 + secs(10) + ms(50));
    assert!(agent_msgs(&e.tick(t0 + secs(11))).is_empty());
}

#[test]
fn steering_is_only_used_when_the_agent_advertised_it() {
    let t0 = Instant::now();
    let mut e = five_background(
        t0,
        Config {
            interrupt: Some(InterruptMode::Steer),
            ..test_config()
        },
    );
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(2, "hi"), t0 + secs(10) + ms(100));
    assert_eq!(agent_msgs(&out)[0]["method"], "session/prompt");
    assert!(has_note(&out, "did not advertise steering"));
}

#[test]
fn closing_the_session_forgets_steering_state() {
    let t0 = Instant::now();
    let mut e = steering_engine(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    e.on_client(&text_prompt(2, "hi"), t0 + secs(10) + ms(100));
    assert_eq!(e.steers.len(), 1);
    let close = line(
        json!({"jsonrpc": "2.0", "id": 9, "method": "session/close", "params": {"sessionId": "s"}}),
    );
    e.on_client(&close, t0 + secs(11));
    assert!(e.steers.is_empty() && e.handoffs.is_empty());
}
