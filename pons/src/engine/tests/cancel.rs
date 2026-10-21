// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn unknown_and_malformed_lines_pass_through_verbatim() {
    let mut e = engine();
    let t0 = Instant::now();
    for raw in [
        &b"{\"id\": 5,  \"method\":\"x/y\"}"[..],
        b"not json",
        b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"session/request_permission\",\"params\":{}}",
    ] {
        let out = e.on_client(raw, t0);
        assert!(out.contains(&Action::ToAgent(raw.to_vec())));
        let out = e.on_agent(raw, t0);
        assert!(out.contains(&Action::ToClient {
            line: raw.to_vec(),
            synthesized: false
        }));
    }
}

#[test]
fn cancel_dropped_and_prompt_answered_when_subagent_in_flight_and_quiet() {
    let t0 = Instant::now();
    let mut e = busy(t0);
    let out = e.on_client(&cancel(), t0 + Duration::from_secs(3));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
    let msgs = client_lines(&out);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["id"], 1);
    assert_eq!(msgs[0]["result"]["stopReason"], "cancelled");

    // The agent's eventual answer to prompt 1 is swallowed.
    let late = e.on_agent(&response(1), t0 + Duration::from_secs(9));
    assert!(client_lines(&late).is_empty());
    // The next prompt is forwarded as usual and its response is not.
    let p2 = prompt(2);
    assert_eq!(
        e.on_client(&p2, t0 + Duration::from_secs(10)),
        vec![Action::ToAgent(p2)]
    );
    assert_eq!(
        client_lines(&e.on_agent(&response(2), t0 + Duration::from_secs(11))).len(),
        1
    );
}

#[test]
fn repeated_cancel_within_window_is_forwarded_and_resets_tracking() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut e = busy(t0);
    let out = e.on_client(&cancel(), t0 + ms(3000));
    assert_eq!(client_lines(&out).len(), 1);
    // A second Stop 2.2s after the dropped one, no prompt in between.
    let c = cancel();
    let out = e.on_client(&c, t0 + ms(5200));
    assert!(out.contains(&Action::ToAgent(c.clone())));
    assert!(matches!(&out[0], Action::Note(n) if n.contains("stop escape")));
    // Tracking is reset: another cancel is plain-forwarded, not filtered.
    assert_eq!(
        e.on_client(&c, t0 + ms(5300)),
        vec![Action::ToAgent(c.clone())]
    );
    // The old prompt's response is still hidden from the client.
    assert!(client_lines(&e.on_agent(&response(1), t0 + ms(5400))).is_empty());
    assert!(e.swallow.is_empty() && e.prompts.is_empty());
}

#[test]
fn a_new_prompt_resets_the_stop_window() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut e = busy(t0);
    let out = e.on_client(&cancel(), t0 + ms(3000));
    assert_eq!(client_lines(&out).len(), 1);
    // Zed's next prompt, then a cancel 2.2s after the dropped one: not a
    // Stop (the window belongs to the earlier turn), filtered again.
    e.on_client(&prompt(2), t0 + ms(3100));
    assert_eq!(e.sessions["s"].dropped_cancel, None);
    let out = e.on_client(&cancel(), t0 + ms(5200));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
    assert!(
        !out.iter()
            .any(|a| matches!(a, Action::Note(n) if n.contains("stop escape")))
    );
    assert_eq!(client_lines(&out)[0]["result"]["stopReason"], "cancelled");
}

#[test]
fn a_late_swallowed_response_leaves_the_new_turns_state_alone() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut e = busy(t0);
    e.on_client(&cancel(), t0 + ms(3000)); // prompt 1 answered as cancelled
    e.on_client(&prompt(2), t0 + ms(3100));
    e.on_agent(
        &child(json!({"toolCallId": "c2", "title": "Read b"})),
        t0 + ms(3200),
    );
    let out = e.on_agent(&response(1), t0 + ms(3300));
    // Hidden from the client, with nothing of turn 2 touched.
    assert!(client_lines(&out).is_empty());
    assert!(
        out.iter()
            .any(|a| matches!(a, Action::Note(n) if n.contains("state is left alone")))
    );
    let s = &e.sessions["s"];
    assert_eq!(s.prompt_id, Some(json!(2)));
    assert!(s.in_flight.contains("P") && s.parents.contains("P"));
    assert!(e.prompts.contains_key("2") && e.swallow.is_empty());
    // The folded log of turn 2 is still pending, not flushed or cleared.
    let out = e.on_agent(&response(2), t0 + ms(3400));
    assert_eq!(client_lines(&out).last().unwrap()["id"], 2);
}

#[test]
fn a_new_prompt_clears_a_stale_dropped_cancel() {
    let t0 = Instant::now();
    let mut e = busy(t0);
    e.on_client(&cancel(), t0 + Duration::from_millis(3000));
    assert!(e.sessions["s"].dropped_cancel.is_some());
    e.on_client(&prompt(2), t0 + Duration::from_millis(3001));
    assert!(e.sessions["s"].dropped_cancel.is_none());
}

#[test]
fn closing_a_session_drops_all_its_state_and_silences_it() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid("A", "a1", "Read a"), t0 + secs(2));
    e.on_client(&cancel(), t0 + secs(5)); // dropped: swallow entry
    e.on_agent(&chunk("x"), t0 + secs(5));
    let close = line(
        json!({"jsonrpc": "2.0", "id": 9, "method": "session/close", "params": {"sessionId": "s"}}),
    );
    let out = e.on_client(&close, t0 + secs(6));
    assert!(out.contains(&Action::ToAgent(close.clone())));
    assert!(e.sessions.is_empty() && e.prompts.is_empty() && e.swallow.is_empty());
    assert!(e.held.is_empty() && e.folds.background_sessions().is_empty());
    // No ticks, no cards, no plans for it any more; its updates pass.
    assert!(e.tick(t0 + secs(100)).is_empty());
    let raw = kid("A", "a2", "Read a2");
    assert_eq!(
        e.on_agent(&raw, t0 + secs(101)),
        vec![Action::ToClient {
            line: raw,
            synthesized: false
        }]
    );
    assert!(e.tick(t0 + secs(200)).is_empty());
    // Loading it again makes it live.
    e.on_client(&load_request("session/load"), t0 + secs(300));
    assert!(!e.closed.contains("s"));
}

#[test]
fn repeated_cancel_after_window_is_dropped_again() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut e = busy(t0);
    e.on_client(&cancel(), t0 + ms(3000));
    e.on_client(&prompt(2), t0 + ms(3100));
    let out = e.on_client(&cancel(), t0 + ms(6100));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
    assert_eq!(client_lines(&out)[0]["id"], 2);
}

#[test]
fn zero_stop_window_never_forwards_repeated_cancel() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut e = busy(t0);
    e.cfg.stop_window = Duration::ZERO;
    e.on_client(&cancel(), t0 + ms(3000));
    e.on_client(&prompt(2), t0 + ms(3100));
    let out = e.on_client(&cancel(), t0 + ms(5200));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
}

#[test]
fn cancel_is_held_whether_or_not_the_main_agent_is_streaming() {
    let t0 = Instant::now();
    let mut e = busy(t0);
    e.on_agent(&update(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}})), t0 + Duration::from_secs(2));
    let out = e.on_client(&cancel(), t0 + Duration::from_millis(2010));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
    assert_eq!(client_lines(&out)[0]["result"]["stopReason"], "cancelled");
}

#[test]
fn parent_updates_do_not_count_as_main_activity() {
    let t0 = Instant::now();
    let mut e = busy(t0);
    e.on_agent(
        &update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "P", "content": []})),
        t0 + Duration::from_secs(2),
    );
    let out = e.on_client(&cancel(), t0 + Duration::from_secs(3));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
}

#[test]
fn cancel_forwarded_after_subagent_completes_or_without_filter() {
    let t0 = Instant::now();
    let mut e = busy(t0);
    e.on_agent(
        &update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "P", "status": "completed"}),
        ),
        t0,
    );
    let c = cancel();
    assert_eq!(
        e.on_client(&c, t0 + Duration::from_secs(5)),
        vec![Action::ToAgent(c.clone())]
    );

    let mut off = Engine::new(Config {
        cancel_filter: false,
        stop_window: Duration::ZERO,
        subagent_idle: Duration::ZERO,
        ..Config::default()
    });
    off.on_client(&prompt(1), t0);
    off.on_agent(&child(json!({})), t0);
    assert_eq!(
        off.on_client(&c, t0 + Duration::from_secs(5)),
        vec![Action::ToAgent(c)]
    );
}

#[test]
fn child_updates_are_folded_not_forwarded() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&task_call(), t0);
    let out = e.on_agent(&child(json!({})), t0);
    let msgs = client_lines(&out);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["params"]["update"]["toolCallId"], "P");
    assert!(msgs[0]["params"]["update"]["_meta"].is_null());
    assert!(matches!(
        &out[0],
        Action::ToClient {
            synthesized: true,
            ..
        }
    ));
}

#[test]
fn a_child_tool_call_start_is_noted_with_its_card_and_clipped_title() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&task_call(), t0);
    let title = format!("Bash {}", "x".repeat(200));
    let out = e.on_agent(&child(json!({"title": title})), t0);
    assert_eq!(
        notes(&out),
        vec![format!(
            "subagent card P: child tool call c1 started: Bash {}...",
            "x".repeat(72)
        )]
    );
}

#[test]
fn terminal_parent_flushes_fold_before_forwarded_update() {
    let t0 = Instant::now();
    let mut e = busy(t0);
    e.on_agent(
        &child(json!({"toolCallId": "c2", "title": "Bash: cargo test"})),
        t0 + Duration::from_millis(10),
    );
    let done = update(
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "P", "status": "completed"}),
    );
    let out = e.on_agent(&done, t0 + Duration::from_millis(20));
    assert_eq!(out.len(), 2);
    assert!(matches!(
        &out[0],
        Action::ToClient {
            synthesized: true,
            ..
        }
    ));
    assert_eq!(
        out[1],
        Action::ToClient {
            line: done,
            synthesized: false
        }
    );
    assert!(
        client_lines(&out)[0]
            .to_string()
            .contains("Bash: cargo test")
    );
}

#[test]
fn fold_disabled_forwards_child_updates_flat() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        fold: false,
        stop_window: Duration::ZERO,
        subagent_idle: Duration::ZERO,
        ..Config::default()
    });
    let c = child(json!({}));
    assert_eq!(
        e.on_agent(&c, t0),
        vec![Action::ToClient {
            line: c,
            synthesized: false
        }]
    );
}
