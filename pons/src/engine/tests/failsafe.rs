// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn passthrough_still_swallows_the_answer_to_a_dropped_cancel() {
    let t0 = Instant::now();
    let mut e = Engine::new(failing_config(1));
    e.on_client(&prompt(1), t0);
    e.on_agent(&task_call(), t0);
    e.on_agent(&child(json!({})), t0);
    let out = e.on_client(&cancel(), t0 + secs(3));
    assert_eq!(client_lines(&out).len(), 1, "answered as cancelled");
    let out = e.on_agent(&chunk("boom"), t0 + secs(4));
    assert!(notes(&out)[0].contains("pure passthrough"));
    // The late answer to prompt 1 must not reach the client again.
    let out = e.on_agent(&response(1), t0 + secs(5));
    assert!(client_lines(&out).is_empty(), "{out:?}");
    assert!(e.swallow.is_empty());
    // Other responses still pass.
    let out = e.on_agent(&response(7), t0 + secs(6));
    assert_eq!(client_lines(&out).len(), 1);
}

/// Replays the shapes of the false finishes in a real session (card
/// toolu_01McEgUjs87WRP1WBnAbtimq of a 165eb299 log, finished wrongly at
/// +531s, +591s and +641s): each time the last child event was a tool
/// completion, followed by silent thinking. Offsets in seconds from the
/// launch, embedded because the log is not part of the repository.
#[test]
fn replay_of_the_real_false_finishes_finishes_nothing() {
    // (offset, kind): 'o' opens a tool, 'c' completes it, 'm' main text.
    let events = [
        (3, 'o'),
        (9, 'c'),
        (14, 'o'),
        (31, 'c'),
        (36, 'o'),
        (80, 'c'),
        (85, 'o'),
        (140, 'c'),
        (141, 'o'),
        (260, 'c'),
        (262, 'o'),
        (509, 'c'), // silent until +531: the first false finish
        (532, 'o'),
        (570, 'c'), // silent until +591: the second
        (592, 'o'),
        (619, 'c'), // silent until +641: the third
        (642, 'o'),
        (700, 'c'),
    ];
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    let mut next = 0;
    let mut open = 0;
    for at in 1..=760u64 {
        let now = t0 + secs(at);
        while next < events.len() && events[next].0 == at {
            let id = format!("t{next}");
            let raw = if events[next].1 == 'o' {
                open += 1;
                kid("A", &id, "Bash: build")
            } else {
                open -= 1;
                kid_end("A", &format!("t{}", next - 1))
            };
            let out = e.on_agent(&raw, now);
            assert!(!out.iter().any(is_finish_note), "event at {at}: {out:?}");
            next += 1;
        }
        let out = e.tick(now);
        assert!(!out.iter().any(is_finish_note), "tick at {at}: {out:?}");
        assert!(entries(&out).is_empty(), "entry at {at}");
        assert!(
            updates_for(&out, "A")
                .iter()
                .all(|m| m["params"]["update"]["status"] == "in_progress"),
            "finished at {at}"
        );
    }
    assert_eq!(open, 0);
    assert!(!e.folds.is_finished("A"));
}

#[test]
fn a_failing_rule_forwards_the_original_line_and_the_session_continues() {
    let t0 = Instant::now();
    let mut e = Engine::new(failing_config(5));
    let raw = boom_request();
    let out = e.on_client(&raw, t0);
    assert_eq!(out.len(), 2);
    assert!(notes(&out)[0].contains("fail-safe"));
    assert!(notes(&out)[0].contains("1 in a row for s"));
    assert_eq!(out[1], Action::ToAgent(raw));
    let boom_chunk = chunk("boom");
    let out = e.on_agent(&boom_chunk, t0);
    assert!(notes(&out)[0].contains("2 in a row for s"));
    assert_eq!(
        out[1],
        Action::ToClient {
            line: boom_chunk,
            synthesized: false
        }
    );
    // Rules still work for the next message.
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    let out = e.on_agent(&real_launch_result("A"), t0);
    assert_eq!(updates_for(&out, "A").len(), 1);
}

#[test]
fn consecutive_failures_switch_the_session_to_passthrough() {
    let t0 = Instant::now();
    let mut e = Engine::new(failing_config(3));
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    for i in 1..=3 {
        let out = e.on_agent(&chunk("boom"), t0);
        assert_eq!(notes(&out)[0].contains("switching"), i == 3, "{i}");
    }
    // Passthrough: no rewrite, no note, other sessions are unaffected.
    let launch = real_launch_result("A");
    assert_untouched(&e.on_agent(&launch, t0 + secs(1)), &launch);
    let cancel_raw = cancel();
    assert_eq!(
        e.on_client(&cancel_raw, t0 + secs(9)),
        vec![Action::ToAgent(cancel_raw)]
    );
    let other = line(json!({"jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": "other", "update": {"sessionUpdate": "tool_call",
        "toolCallId": "P", "title": "Task", "kind": "think", "name": "Agent"}}}));
    assert_eq!(sent_to_client(&e.on_agent(&other, t0)).len(), 1);
    assert!(e.guard.is_passthrough(Some("s")) && !e.guard.is_passthrough(Some("other")));
}

#[test]
fn a_success_between_failures_resets_the_count() {
    let t0 = Instant::now();
    let mut e = Engine::new(failing_config(2));
    e.on_agent(&chunk("boom"), t0);
    e.on_agent(&chunk("fine"), t0);
    let out = e.on_agent(&chunk("boom"), t0);
    assert!(!notes(&out)[0].contains("switching"));
    assert!(!e.guard.is_passthrough(Some("s")));
}
