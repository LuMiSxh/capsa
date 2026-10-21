// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn finish_entry_uses_retitled_card_and_last_agent_text() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(
        &update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "A", "title": "Review the diff"})),
        t0 + secs(1),
    );
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(2));
    e.on_agent(
        &update(json!({"_meta": {"claudeCode": {"parentToolUseId": "A"}},
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "y".repeat(500)}})),
        t0 + secs(3),
    );
    let out = e.tick(t0 + secs(23));
    let n = &entries(&out)[0]["params"]["update"];
    assert_eq!(n["title"], "Review the diff finished");
    let text = n["content"][0]["content"]["text"].as_str().unwrap();
    let (calls, snippet) = text.split_once('\n').unwrap();
    assert_eq!(calls, "1 tool call");
    assert_eq!(snippet.chars().count(), 500);
}

#[test]
fn finish_entry_can_be_disabled() {
    let t0 = Instant::now();
    let mut e = engine();
    e.folds = Folds::new(FOLD_THROTTLE).with_idle(secs(20));
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&child_text("A", "done"), t0);
    let out = e.tick(t0 + secs(21));
    assert_eq!(updates_for(&out, "A").len(), 1);
    assert!(entries(&out).is_empty());
}

#[test]
fn a_reopened_card_updates_its_one_entry_instead_of_adding_another() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid_ended("B", "b1", "Read b"), t0 + secs(2));
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(5));
    let id = |m: &Value| m["params"]["update"]["toolCallId"].clone();
    let out = e.on_agent(&task_notification(), t0 + secs(6));
    let first = entries(&out);
    assert_eq!(id(&first[0]), "pons-finished-B");
    assert_eq!(first[0]["params"]["update"]["sessionUpdate"], "tool_call");
    // B was alive after all: its sent entry goes back to running.
    let out = e.on_agent(&kid_ended("B", "b2", "Read b2"), t0 + secs(10));
    let resumed = entries(&out);
    assert_eq!(resumed.len(), 1);
    let u = &resumed[0]["params"]["update"];
    assert_eq!(u["toolCallId"], "pons-finished-B");
    assert_eq!(u["sessionUpdate"], "tool_call_update");
    assert_eq!(u["status"], "in_progress");
    let out = e.tick(t0 + secs(11));
    assert_eq!(id(&entries(&out)[0]), "pons-finished-A");
    // B says its last words and goes idle: the same entry is completed.
    e.on_agent(&child_text("B", "Final answer."), t0 + secs(12));
    let out = e.tick(t0 + secs(40));
    let b = entries(&out);
    assert_eq!(b.len(), 1);
    let u = &b[0]["params"]["update"];
    assert_eq!(u["toolCallId"], "pons-finished-B");
    assert_eq!(u["sessionUpdate"], "tool_call_update");
    assert_eq!(u["status"], "completed");
    assert_eq!(
        u["content"][0]["content"]["text"],
        "2 tool calls\nFinal answer."
    );
    // Never a second entry for the card.
    let sent: Vec<_> = [first, resumed, entries(&out)].concat();
    assert!(
        sent.iter()
            .filter(|m| m["params"]["update"]["sessionUpdate"] == "tool_call")
            .count()
            == 1
    );
}

#[test]
fn provisional_finishes_send_no_notice_and_keep_the_parent_in_flight() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        notices: NoticeMode::Always,
        ..test_config()
    });
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&child_text("A", "Done."), t0 + secs(1));
    let out = e.tick(t0 + secs(21));
    assert!(e.folds.is_finished("A"));
    assert!(notices_in(&out).is_empty());
    assert!(e.sessions["s"].in_flight.contains("A"));
    // The cancel filter still drops a cancel for it.
    let out = e.on_client(&cancel(), t0 + secs(25));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
    // Later activity: the card is running again with its log.
    let out = e.on_agent(&kid("A", "a2", "Read a2"), t0 + secs(30));
    assert_eq!(
        updates_for(&out, "A")[0]["params"]["update"]["status"],
        "in_progress"
    );
    // A marker then confirms: now the notice goes out.
    e.on_agent(&kid_end("A", "a2"), t0 + secs(31));
    let out = e.on_agent(&task_notification(), t0 + secs(32));
    assert_eq!(notices_in(&out).len(), 1);
    assert!(!e.sessions["s"].in_flight.contains("A"));
}

#[test]
fn a_prompt_response_finishes_the_cards_its_turn_launched() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid("A", "a1", "Write big.rs"), t0 + secs(2)); // tool open
    let out = e.on_agent(&response(1), t0 + secs(5));
    // Cards, then the entries, then the response itself.
    let lines = client_lines(&out);
    let last = lines.last().unwrap();
    assert_eq!(last["id"], 1);
    assert_eq!(entries(&out).len(), 2);
    assert!(
        out.iter()
            .any(|a| matches!(a, Action::Note(n) if n.contains("prompt 1 answered: finishing 2")))
    );
    assert!(e.folds.running_background("s").is_empty());
    assert!(e.sessions["s"].in_flight.is_empty());
    let c = cancel();
    assert_eq!(
        e.on_client(&c, t0 + secs(10)),
        vec![Action::ToAgent(c)],
        "nothing left to protect"
    );
}

#[test]
fn a_cancelled_prompt_response_marks_its_cards_stopped() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    let cancelled = line(json!({"jsonrpc": "2.0", "id": 1, "result": {"stopReason": "cancelled"}}));
    let out = e.on_agent(&cancelled, t0 + secs(5));
    let a = updates_for(&out, "A");
    assert_eq!(a[0]["params"]["update"]["status"], "failed");
}

#[test]
fn a_handoff_response_does_not_finish_the_cards() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    // The user sends again while the subagents run: the adapter answers
    // prompt 1 only because prompt 2 took over.
    e.on_client(&prompt(2), t0 + secs(3));
    let out = e.on_agent(&response(1), t0 + secs(4));
    assert!(entries(&out).is_empty());
    assert!(e.folds.running_background("s").len() == 2);
    assert!(
        out.iter()
            .any(|a| matches!(a, Action::Note(n) if n.contains("handoff")))
    );
    assert_eq!(client_lines(&out).last().unwrap()["id"], 1);
    // They belong to prompt 2.
    let out = e.on_agent(&response(2), t0 + secs(9));
    assert_eq!(entries(&out).len(), 2);
    assert!(e.folds.running_background("s").is_empty());
}

#[test]
fn a_marker_with_no_qualifying_candidate_finishes_nothing() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid("A", "a1", "Write big.rs"), t0 + secs(2));
    e.on_agent(&kid("B", "b1", "Bash: sleep"), t0 + secs(3));
    let out = e.on_agent(&task_notification(), t0 + secs(10));
    assert!(entries(&out).is_empty());
    assert!(updates_for(&out, "A").is_empty() && updates_for(&out, "B").is_empty());
    assert!(
        out.iter()
            .any(|a| matches!(a, Action::Note(n) if n.contains("finished nothing")))
    );
    assert_eq!(e.folds.running_background("s").len(), 2);
}

/// Builds the tick the adapter sends for a running subagent's own card with
/// `parentToolUseId` set to the card itself (log l.1985 of
/// pons-2026-10-16T17-31-14-1038: `toolCallId=FgXE parentToolUseId=FgXE
/// status=in_progress`). No such tick ever ends, so it must not count as
/// an open tool call.
fn self_tick(id: &str) -> Vec<u8> {
    update(
        json!({"_meta": {"claudeCode": {"toolName": "Agent", "parentToolUseId": id}},
        "toolCallId": id, "sessionUpdate": "tool_call_update", "status": "in_progress"}),
    )
}

#[test]
fn a_self_parented_tick_does_not_keep_the_card_from_finishing() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(2));
    e.on_agent(&self_tick("A"), t0 + secs(3));
    e.on_agent(&child_text("A", "Report."), t0 + secs(4));
    // B keeps working through its own tool call.
    e.on_agent(&kid("B", "b1", "Bash: sleep"), t0 + secs(5));
    let out = e.on_agent(&task_notification(), t0 + secs(10));
    let a = updates_for(&out, "A");
    assert_eq!(a.last().unwrap()["params"]["update"]["status"], "completed");
    assert_eq!(entries(&out).len(), 1, "A is announced");
    assert!(updates_for(&out, "B").is_empty(), "B keeps running");
    assert_eq!(e.folds.running_background("s"), vec!["B".to_owned()]);
}

#[test]
fn a_self_parented_tick_is_not_a_tool_line_or_activity() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(2));
    e.on_agent(&child_text("A", "Report."), t0 + secs(3));
    // A tick after the report must not be "activity since" the notification.
    let out = e.on_agent(&self_tick("A"), t0 + secs(4));
    assert!(updates_for(&out, "A").is_empty());
    let out = e.on_agent(&task_notification(), t0 + secs(5));
    let a = updates_for(&out, "A");
    assert_eq!(a.last().unwrap()["params"]["update"]["status"], "completed");
    assert_eq!(
        card_text(a.last().unwrap()),
        "done: Read a\nagent: Report.\ndone - 1 tool call"
    );
}

#[test]
fn a_marker_prefers_the_card_that_ended_with_text() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    // B is quieter, but its last act was a tool result: A wrote its report.
    e.on_agent(&kid_ended("B", "b1", "Read b"), t0 + secs(2));
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(5));
    e.on_agent(&child_text("A", "Report."), t0 + secs(6));
    let out = e.on_agent(&task_notification(), t0 + secs(7));
    assert_eq!(
        updates_for(&out, "A")[0]["params"]["update"]["status"],
        "completed"
    );
    assert!(updates_for(&out, "B").is_empty());
}

#[test]
fn a_retired_card_that_turns_out_alive_stays_a_background_card() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(1));
    e.on_agent(&child_text("A", "Done?"), t0 + secs(1));
    let out = e.tick(t0 + secs(21)); // provisional idle finish
    assert!(!entries(&out).is_empty() || e.held.len() == 1);
    // Main-agent activity goes on meanwhile.
    e.on_agent(&chunk("main"), t0 + secs(22));
    // The subagent was alive: it keeps its log and its card.
    let out = e.on_agent(&kid("A", "a2", "Read a2"), t0 + secs(40));
    let a = updates_for(&out, "A");
    assert_eq!(a[0]["params"]["update"]["status"], "in_progress");
    assert!(
        card_text(&a[0]).starts_with("done: Read a\n"),
        "{}",
        card_text(&a[0])
    );
    assert!(
        a[0]["params"]["update"]["title"]
            .as_str()
            .unwrap()
            .contains("running")
    );
    // And never a second entry id for it.
    let ids: HashSet<String> = entries(&out)
        .iter()
        .chain(entries(&e.tick(t0 + secs(100))).iter())
        .map(|m| {
            m["params"]["update"]["toolCallId"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(ids.iter().all(|i| i == "pons-finished-A"), "{ids:?}");
}

#[test]
fn zero_idle_never_finishes_by_silence() {
    let t0 = Instant::now();
    let mut e = engine();
    e.folds = Folds::new(FOLD_THROTTLE);
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    let out = e.tick(t0 + secs(3600));
    assert!(entries(&out).is_empty());
    assert!(
        updates_for(&out, "A")
            .iter()
            .all(|m| m["params"]["update"]["status"] == "in_progress")
    );
}

#[test]
fn background_subagent_survives_a_handoff() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid("A", "a1", "Read a"), t0 + secs(2));
    // The next prompt takes over: prompt 1 is answered, the cards go on.
    e.on_client(&prompt(2), t0 + secs(3));
    e.on_agent(&response(1), t0 + secs(3));
    let c = cancel();
    let out = e.on_client(&c, t0 + secs(8));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
    let out = e.on_agent(&kid("A", "a2", "Read a2"), t0 + secs(9));
    let a = updates_for(&out, "A");
    assert_eq!(a[0]["params"]["update"]["status"], "in_progress");
    assert_eq!(card_text(&a[0]), "running: Read a\nrunning: Read a2");
}

#[test]
fn stop_escape_marks_running_background_cards_failed() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid("A", "a1", "Read a"), t0 + secs(2));
    e.on_client(&cancel(), t0 + secs(5));
    let out = e.on_client(&cancel(), t0 + secs(6));
    let a = updates_for(&out, "A");
    assert_eq!(a[0]["params"]["update"]["status"], "failed");
    assert_eq!(card_text(&a[0]), "running: Read a\nstopped - 1 tool call");
    let n = entries(&out);
    let ids: Vec<_> = n
        .iter()
        .map(|m| m["params"]["update"]["toolCallId"].clone())
        .collect();
    assert_eq!(ids, ["pons-finished-A", "pons-finished-B"]);
    let u = &n[0]["params"]["update"];
    assert_eq!(u["title"], "Agent stopped");
    assert_eq!(u["status"], "failed");
    assert_eq!(u["kind"], "other");
    assert_eq!(u["content"][0]["content"]["text"], "1 tool call");
    assert!(out.iter().any(|x| matches!(x, Action::ToAgent(_))));
    assert!(e.folds.running_background("s").is_empty());
}

#[test]
fn prompt_response_flushes_folds_and_resets_subagents() {
    let t0 = Instant::now();
    let mut e = busy(t0);
    e.on_agent(
        &child(json!({"toolCallId": "c2", "title": "Read b"})),
        t0 + Duration::from_millis(10),
    );
    let out = e.on_agent(&response(1), t0 + Duration::from_millis(20));
    assert_eq!(
        client_lines(&out).len(),
        2,
        "flushed fold, then the response"
    );
    let c = cancel();
    assert_eq!(
        e.on_client(&c, t0 + Duration::from_secs(5)),
        vec![Action::ToAgent(c)]
    );
}
