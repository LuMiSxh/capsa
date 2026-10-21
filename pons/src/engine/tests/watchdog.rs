// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn the_watchdog_answers_a_turn_that_never_ends_and_swallows_the_late_response() {
    let t0 = Instant::now();
    let mut e = finished_text(t0);
    let stall = secs(120);
    assert!(e.tick(t0 + secs(2) + stall - ms(1)).is_empty());
    let out = e.tick(t0 + secs(2) + stall);
    let answers = responses(&out);
    assert_eq!(answers.len(), 1, "{out:?}");
    assert_eq!(answers[0]["id"], 1);
    assert_eq!(answers[0]["result"]["stopReason"], "end_turn");
    assert!(has_note(&out, "turn watchdog"));
    assert!(e.prompts.is_empty());
    assert_eq!(e.sessions["s"].prompt_id, None);
    // Once only.
    assert!(e.tick(t0 + secs(400)).is_empty());
    // The real response arriving later is dropped.
    let out = e.on_agent(&response(1), t0 + secs(500));
    assert!(responses(&out).is_empty(), "{out:?}");
    assert!(has_note(&out, "swallowed agent response"));
    assert!(e.swallow.is_empty());
}

#[test]
fn the_watchdog_never_fires_while_anything_is_open_or_recent() {
    let t0 = Instant::now();
    let late = t0 + secs(2) + secs(1000);

    // An open tool call.
    let mut e = finished_text(t0);
    e.on_agent(&pending_call("m1"), t0 + secs(3));
    e.on_agent(&usage(), t0 + secs(4));
    assert!(responses(&e.tick(late)).is_empty());

    // A tool call that ended and then silence: the last word is not text.
    let mut e = finished_text(t0);
    e.on_agent(&pending_call("m1"), t0 + secs(3));
    e.on_agent(&tool_status("m1", "completed"), t0 + secs(4));
    assert!(responses(&e.tick(late)).is_empty());

    // Thinking after the text.
    let mut e = finished_text(t0);
    e.on_agent(
        &update(json!({"sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "hmm"}})),
        t0 + secs(3),
    );
    assert!(responses(&e.tick(late)).is_empty());

    // A running subagent.
    let mut e = busy(t0);
    e.on_agent(&chunk("waiting"), t0 + secs(1));
    assert!(responses(&e.tick(late)).is_empty());

    // A question to the client nobody answered.
    let mut e = finished_text(t0);
    let ask = line(
        json!({"jsonrpc": "2.0", "id": "p1", "method": "session/request_permission",
        "params": {"sessionId": "s", "toolCall": {"toolCallId": "m9"}, "options": []}}),
    );
    e.on_agent(&ask, t0 + secs(3));
    assert!(responses(&e.tick(late)).is_empty());
    // The answer is activity too: the window starts again from it.
    e.on_client(
        &line(
            json!({"jsonrpc": "2.0", "id": "p1", "result": {"outcome": {"outcome": "cancelled"}}}),
        ),
        late,
    );
    assert!(responses(&e.tick(late + secs(119))).is_empty());
    assert_eq!(responses(&e.tick(late + secs(120))).len(), 1);

    // Any update inside the window moves it.
    let mut e = finished_text(t0);
    e.on_agent(&chunk("more"), t0 + secs(100));
    assert!(responses(&e.tick(t0 + secs(100) + secs(119))).is_empty());
    assert_eq!(responses(&e.tick(t0 + secs(100) + secs(120))).len(), 1);
}

#[test]
fn the_watchdog_can_be_disabled_and_needs_an_outstanding_prompt() {
    let t0 = Instant::now();
    let cfg = Config {
        turn_stall: Duration::ZERO,
        ..test_config()
    };
    let mut e = Engine::new(cfg);
    e.on_client(&prompt(1), t0);
    e.on_agent(&chunk("done"), t0 + secs(1));
    assert!(e.tick(t0 + secs(10_000)).is_empty());

    let mut e = finished_text(t0);
    e.on_agent(&response(1), t0 + secs(3));
    assert!(e.tick(t0 + secs(10_000)).is_empty());
}

#[test]
fn a_stop_closes_the_open_tool_calls_once_but_not_the_subagents() {
    let t0 = Instant::now();
    let mut e = five_background(t0, test_config());
    e.on_agent(&pending_call("m1"), t0 + secs(9));
    e.on_agent(&pending_call("m2"), t0 + secs(9));
    e.on_agent(&tool_status("m2", "in_progress"), t0 + secs(9));
    e.on_agent(&pending_call("m3"), t0 + secs(9));
    e.on_agent(&tool_status("m3", "completed"), t0 + secs(9));
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.tick(t0 + secs(11));
    assert!(has_note(&out, "stop:"));
    assert_eq!(
        failed_ids(&out)
            .iter()
            .filter(|i| i.starts_with('m'))
            .count(),
        2
    );
    assert!(failed_ids(&out).contains(&"m1".to_owned()));
    assert!(failed_ids(&out).contains(&"m2".to_owned()));
    assert!(!failed_ids(&out).contains(&"m3".to_owned()));
    assert!(has_note(&out, "closing 2 open tool call(s)"));
    // The late cancelled response closes nothing again.
    let out = e.on_agent(&cancelled_response(1), t0 + secs(12));
    assert!(failed_ids(&out).iter().all(|i| !i.starts_with('m')));
}

#[test]
fn a_message_sent_while_subagents_run_closes_no_tool_call() {
    let t0 = Instant::now();
    let mut e = five_background(t0, test_config());
    e.on_agent(&pending_call("m1"), t0 + secs(9));
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&text_prompt(2, "hello"), t0 + secs(10) + ms(100));
    assert!(failed_ids(&out).is_empty());
    assert!(failed_ids(&e.tick(t0 + secs(12))).is_empty());
    assert!(e.sessions["s"].open_tools.contains_key("m1"));
}

#[test]
fn a_cancelled_response_closes_the_open_tool_calls_before_it() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&pending_call("m1"), t0);
    e.on_agent(&pending_call("m2"), t0);
    e.on_agent(&tool_status("m2", "failed"), t0);
    e.on_client(&cancel(), t0 + secs(1));
    let out = e.on_agent(&cancelled_response(1), t0 + secs(2));
    let lines = client_lines(&out);
    assert_eq!(lines.len(), 2, "{out:?}");
    assert_eq!(lines[0]["params"]["update"]["toolCallId"], "m1");
    assert_eq!(lines[0]["params"]["update"]["status"], "failed");
    assert_eq!(
        lines[0]["params"]["update"]["sessionUpdate"],
        "tool_call_update"
    );
    assert_eq!(lines[1]["result"]["stopReason"], "cancelled");
    assert!(e.sessions["s"].open_tools.is_empty());
}

#[test]
fn an_end_turn_response_leaves_the_tool_calls_alone_and_the_flag_turns_it_off() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&pending_call("m1"), t0);
    assert!(failed_ids(&e.on_agent(&response(1), t0 + secs(1))).is_empty());

    let cfg = Config {
        close_ghost_calls: false,
        ..test_config()
    };
    let mut e = Engine::new(cfg);
    e.on_client(&prompt(1), t0);
    e.on_agent(&pending_call("m1"), t0);
    let out = e.on_agent(&cancelled_response(1), t0 + secs(1));
    assert!(failed_ids(&out).is_empty());
}

#[test]
fn a_handoff_after_a_cancel_closes_only_the_old_turns_calls() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&pending_call("m1"), t0);
    e.on_client(&cancel(), t0 + secs(1));
    e.on_client(&prompt(2), t0 + secs(1) + ms(100));
    e.on_agent(&pending_call("m2"), t0 + secs(2));
    let out = e.on_agent(&cancelled_response(1), t0 + secs(3));
    assert_eq!(failed_ids(&out), ["m1"]);
    assert!(e.sessions["s"].open_tools.contains_key("m2"));
}
