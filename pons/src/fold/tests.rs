// SPDX-License-Identifier: MPL-2.0

use super::*;

fn tool_call(id: &str, title: &str) -> Value {
    json!({"sessionUpdate": "tool_call", "toolCallId": id, "title": title, "status": "pending"})
}

fn tool_done(id: &str) -> Value {
    json!({"sessionUpdate": "tool_call_update", "toolCallId": id, "status": "completed"})
}

fn text_chunk(text: &str) -> Value {
    json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}})
}

fn log_text(msg: &Value) -> String {
    let content = &msg["params"]["update"]["content"];
    content[content.as_array().unwrap().len() - 1]["content"]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// A background card `P` launched at `t0`.
fn launched(t0: Instant) -> Folds {
    let mut folds = Folds::new(Duration::ZERO);
    folds.register_parent("s", &json!({"toolCallId": "P", "title": "Review the diff"}));
    folds.launch_background("s", "P", &json!({"toolCallId": "P"}), Some("1"), t0);
    folds
}

/// Like `launched`, with the silence rule on (20 s, cap 600 s) and
/// entries on.
fn watched(t0: Instant) -> Folds {
    let mut folds = Folds::new(Duration::ZERO)
        .with_idle(secs(20))
        .with_max_quiet(secs(600))
        .with_finish_entry(true);
    folds.register_parent("s", &json!({"toolCallId": "P", "title": "Review the diff"}));
    folds.launch_background("s", "P", &json!({"toolCallId": "P"}), Some("1"), t0);
    folds
}

#[test]
fn renders_tool_lines_status_and_text_tail() {
    let mut e = FoldEntry::new("s", "P");
    e.apply_child(&tool_call("c1", "Read src/a.rs"));
    e.apply_child(&tool_done("c1"));
    e.apply_child(&tool_call("c2", "Bash: cargo test"));
    e.apply_child(&text_chunk("looking "));
    e.apply_child(&text_chunk("good"));
    assert_eq!(
        e.render(),
        "done: Read src/a.rs\nrunning: Bash: cargo test\nagent: looking good"
    );
}

#[test]
fn caps_lines_and_truncates_text() {
    let mut e = FoldEntry::new("s", "P");
    for i in 0..50 {
        e.apply_child(&tool_call(&format!("c{i}"), &format!("Tool {i}")));
    }
    e.apply_child(&text_chunk(&"x".repeat(1000)));
    let lines: Vec<_> = e.render().lines().map(str::to_owned).collect();
    assert_eq!(lines.len(), MAX_LINES);
    assert_eq!(lines[0], "running: Tool 21");
    assert_eq!(lines[MAX_LINES - 2], "running: Tool 49");
    let last = &lines[MAX_LINES - 1];
    assert_eq!(last.chars().count(), "agent: ".len() + MAX_TEXT_CHARS);
    assert!(last.starts_with("agent: ..."));
    // The open tools are tracked apart from the capped lines.
    assert_eq!(e.open_tools.len(), 50);
}

#[test]
fn open_tools_and_last_kind_follow_the_wire() {
    let mut e = FoldEntry::new("s", "P");
    assert_eq!(e.last_kind, Kind::Launched);
    e.apply_child(&tool_call("c1", "Read a"));
    assert_eq!((e.last_kind, e.open_tools.len()), (Kind::ToolOpen, 1));
    // A title or content update without a status changes nothing.
    e.apply_child(
        &json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "title": "Read a.rs"}),
    );
    assert_eq!((e.last_kind, e.open_tools.len()), (Kind::ToolOpen, 1));
    e.apply_child(&tool_done("c1"));
    assert_eq!((e.last_kind, e.open_tools.len()), (Kind::ToolResult, 0));
    e.apply_child(
        &json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hm"}}),
    );
    assert_eq!(e.last_kind, Kind::Thought);
    e.apply_child(&text_chunk("done"));
    assert_eq!(e.last_kind, Kind::Text);
    // A tool that arrives already failed never opens.
    e.apply_child(&json!({"sessionUpdate": "tool_call", "toolCallId": "c2", "title": "Bash", "status": "failed"}));
    assert_eq!((e.last_kind, e.open_tools.len()), (Kind::ToolResult, 0));
}

#[test]
fn clips_with_ascii_dots() {
    assert_eq!(clip_head("abcdefghij", 8), "abcde...");
    assert_eq!(clip_tail("abcdefghij", 8), "...fghij");
    assert_eq!(clip_head("abc", 8), "abc");
}

#[test]
fn throttles_and_flushes() {
    let t0 = Instant::now();
    let mut folds = Folds::new(Duration::from_millis(300));
    let first = folds.child_update("s", "P", &tool_call("c1", "Read a"), t0);
    let msg = first.first().expect("first update is emitted at once");
    assert_eq!(msg["params"]["update"]["toolCallId"], "P");
    assert_eq!(msg["params"]["update"]["sessionUpdate"], "tool_call_update");
    assert!(
        msg["params"]["update"]["title"].is_null(),
        "foreground cards keep the adapter's title"
    );
    let held = folds.child_update(
        "s",
        "P",
        &tool_call("c2", "Read b"),
        t0 + Duration::from_millis(100),
    );
    assert!(held.is_empty());
    assert!(folds.tick(t0 + Duration::from_millis(200)).is_empty());
    let ticked = folds.tick(t0 + Duration::from_millis(300));
    assert_eq!(ticked.len(), 1);
    assert_eq!(log_text(&ticked[0]), "running: Read a\nrunning: Read b");
    assert!(folds.tick(t0 + Duration::from_secs(5)).is_empty());
}

#[test]
fn terminal_parent_update_flushes_pending_log_first() {
    let t0 = Instant::now();
    let mut folds = Folds::new(Duration::from_millis(300));
    folds.register_parent(
            "s",
            &json!({"toolCallId": "P", "content": [{"type": "content", "content": {"type": "text", "text": "prompt"}}]}),
        );
    folds.child_update("s", "P", &tool_call("c1", "Read a"), t0);
    folds.child_update(
        "s",
        "P",
        &tool_call("c2", "Read b"),
        t0 + Duration::from_millis(10),
    );
    let done = json!({"toolCallId": "P", "status": "completed"});
    let flushed = folds
        .parent_update("P", &done, true, t0 + Duration::from_millis(20))
        .expect("pending log flushed");
    assert_eq!(log_text(&flushed), "running: Read a\nrunning: Read b");
    let content = flushed["params"]["update"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2, "base content plus log");
    assert_eq!(content[0]["content"]["text"], "prompt");
}

#[test]
fn nested_subagents_fold_into_top_level_parent() {
    let t0 = Instant::now();
    let mut folds = Folds::new(Duration::ZERO);
    folds.child_update("s", "P", &tool_call("child-task", "Agent"), t0);
    assert_eq!(folds.resolve("child-task"), "P");
    assert_eq!(folds.resolve("other"), "other");
}

#[test]
fn clear_session_forgets_entries() {
    let t0 = Instant::now();
    let mut folds = Folds::new(Duration::ZERO);
    folds.child_update("s", "P", &tool_call("c1", "Read a"), t0);
    folds.clear_session("s");
    assert!(folds.flush_session("s", t0).is_empty());
    assert_eq!(folds.resolve("c1"), "c1");
}

#[test]
fn forget_session_drops_cards_notifications_and_roots() {
    let t0 = Instant::now();
    let mut folds = launched(t0);
    folds.child_update("s", "P", &tool_call("c1", "Read a"), t0);
    folds.note_notification("s", t0);
    folds.forget_session("s");
    assert!(folds.roster("s", t0).is_empty());
    assert!(folds.poll(t0 + secs(100)).is_empty());
    assert_eq!(folds.resolve("c1"), "c1");
}

#[test]
fn running_card_title_counts_and_steps_the_elapsed_time() {
    let t0 = Instant::now();
    let mut folds = launched(t0);
    let launch = folds.launch_background("s", "P", &json!({"toolCallId": "P"}), Some("1"), t0);
    assert_eq!(launch["title"], "Review the diff: running 0s");
    let m = folds
        .child_update("s", "P", &tool_call("c1", "Read src/a.rs"), t0 + secs(7))
        .remove(0);
    assert_eq!(
        m["params"]["update"]["title"],
        "Review the diff: running 5s, 1 tool call, last: Read src/a.rs"
    );
    let m = folds
        .child_update("s", "P", &tool_call("c2", "Read src/b.rs"), t0 + secs(74))
        .remove(0);
    assert_eq!(
        m["params"]["update"]["title"],
        "Review the diff: running 1m 10s, 2 tool calls, last: Read src/b.rs"
    );
}

#[test]
fn refresh_ticks_send_the_title_only() {
    let t0 = Instant::now();
    let mut folds = launched(t0);
    assert!(folds.tick(t0 + secs(4)).is_empty());
    let ticked = folds.tick(t0 + secs(5));
    assert_eq!(ticked.len(), 1);
    let u = &ticked[0]["params"]["update"];
    assert_eq!(u["title"], "Review the diff: running 5s");
    assert_eq!(u["status"], "in_progress");
    assert!(u.get("content").is_none(), "no content on a refresh: {u}");
    assert!(folds.tick(t0 + secs(9)).is_empty());
    assert_eq!(folds.tick(t0 + secs(10)).len(), 1);
    // A change of the log still sends the full content.
    let m = folds
        .child_update("s", "P", &tool_call("c1", "Read a"), t0 + secs(11))
        .remove(0);
    assert!(m["params"]["update"]["content"].is_array());
}

#[test]
fn a_silent_running_card_says_quiet_in_title_and_roster() {
    let t0 = Instant::now();
    let mut folds = launched(t0);
    folds.child_update("s", "P", &tool_call("c1", "Read a"), t0 + secs(2));
    let ticked = folds.tick(t0 + secs(37));
    assert_eq!(
        ticked[0]["params"]["update"]["title"],
        "Review the diff: running 35s, 1 tool call, last: Read a, quiet 35s"
    );
    assert_eq!(ticked[0]["params"]["update"]["status"], "in_progress");
    let item = &folds.roster("s", t0 + secs(37))[0];
    assert_eq!(
        (item.content.as_str(), item.status),
        (
            "Review the diff: Read a (1 tool call, 35s, quiet 35s)",
            "in_progress"
        )
    );
    // Below the threshold nothing is said.
    let item = &folds.roster("s", t0 + secs(8))[0];
    assert!(!item.content.contains("quiet"), "{}", item.content);
}

#[test]
fn finished_card_title_and_roster() {
    let t0 = Instant::now();
    let mut folds = launched(t0);
    folds.child_update("s", "P", &tool_call("c1", "Read a"), t0 + secs(2));
    folds.child_update("s", "P", &tool_done("c1"), t0 + secs(2));
    let item = &folds.roster("s", t0 + secs(3))[0];
    assert_eq!(
        (item.content.as_str(), item.status),
        ("Review the diff: Read a (1 tool call, 0s)", "in_progress")
    );
    folds.note_notification("s", t0 + secs(12));
    let done = folds.poll(t0 + secs(12));
    let update = done[0].update.as_ref().unwrap();
    assert_eq!(
        update["params"]["update"]["title"],
        "Review the diff: done, 1 tool call, 12s"
    );
    assert_eq!(update["params"]["update"]["status"], "completed");
    let item = &folds.roster("s", t0 + secs(30))[0];
    assert_eq!(
        (item.content.as_str(), item.status),
        ("Review the diff: done (1 tool call, 12s)", "completed")
    );
    assert!(folds.tick(t0 + secs(30)).is_empty(), "no refresh once done");
}

#[test]
fn roster_item_before_any_activity_is_pending() {
    let t0 = Instant::now();
    let folds = launched(t0);
    let item = &folds.roster("s", t0)[0];
    assert_eq!(
        (item.content.as_str(), item.status),
        ("Review the diff: starting (0 tool calls, 0s)", "pending")
    );
}

#[test]
fn finished_cards_are_retired_by_the_next_turn_or_after_a_minute() {
    let t0 = Instant::now();
    let mut folds = launched(t0);
    folds.note_notification("s", t0 + secs(3));
    folds.poll(t0 + secs(3));
    // The turn in which it finished ends: it stays.
    folds.retire("s", Some(t0));
    folds.clear_session("s");
    assert_eq!(folds.roster("s", t0 + secs(4)).len(), 1);
    // A turn that began after the finish ends: gone.
    let gone = folds.retire("s", Some(t0 + secs(5)));
    assert_eq!(gone, vec![("s".to_owned(), "P".to_owned())]);
    assert!(folds.roster("s", t0 + secs(6)).is_empty());

    let mut folds = launched(t0);
    folds.note_notification("s", t0 + secs(3));
    folds.poll(t0 + secs(3));
    assert!(folds.retire_expired(t0 + secs(62)).is_empty());
    assert_eq!(folds.roster("s", t0 + secs(62)).len(), 1);
    assert_eq!(folds.retire_expired(t0 + secs(63)).len(), 1);
    assert!(folds.roster("s", t0 + secs(63)).is_empty());
}

// The silence rule.

fn reasons(done: &[Finished]) -> Vec<(&str, bool)> {
    done.iter().map(|f| (f.reason, f.confirmed)).collect()
}

#[test]
fn silence_never_finishes_while_a_tool_is_open_or_right_after_a_result() {
    let t0 = Instant::now();
    let mut folds = watched(t0);
    folds.child_update("s", "P", &tool_call("c1", "Write big.rs"), t0 + secs(1));
    assert!(folds.poll(t0 + secs(500)).is_empty(), "tool open");
    folds.child_update("s", "P", &tool_done("c1"), t0 + secs(501));
    assert!(folds.poll(t0 + secs(560)).is_empty(), "after a tool result");
    folds.child_update(
        "s",
        "P",
        &json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hm"}}),
        t0 + secs(561),
    );
    assert!(folds.poll(t0 + secs(700)).is_empty(), "after thinking");
}

#[test]
fn silence_finishes_provisionally_after_final_text_with_nothing_open() {
    let t0 = Instant::now();
    let mut folds = watched(t0);
    folds.child_update("s", "P", &tool_call("c1", "Read a"), t0 + secs(1));
    folds.child_update("s", "P", &tool_done("c1"), t0 + secs(2));
    folds.child_update("s", "P", &text_chunk("All clear."), t0 + secs(3));
    assert!(folds.poll(t0 + secs(22)).is_empty());
    let done = folds.poll(t0 + secs(23));
    assert_eq!(reasons(&done), vec![("idle", false)]);
    assert!(done[0].update.is_some());
    assert!(matches!(done[0].entry, Some(EntryOut::Append(_))));
    assert!(folds.is_finished("P"));
    assert!(folds.poll(t0 + secs(100)).is_empty(), "finished once");
}

#[test]
fn without_visible_text_only_the_long_cap_finishes() {
    let t0 = Instant::now();
    let mut folds = watched(t0);
    folds.set_text_visible(false);
    folds.child_update("s", "P", &tool_call("c1", "Read a"), t0 + secs(1));
    folds.child_update("s", "P", &tool_done("c1"), t0 + secs(2));
    assert!(folds.poll(t0 + secs(300)).is_empty(), "idle does not apply");
    assert!(folds.poll(t0 + secs(601)).is_empty());
    let done = folds.poll(t0 + secs(602));
    assert_eq!(reasons(&done), vec![("quiet-cap", false)]);
    assert_eq!(
        log_text(done[0].update.as_ref().unwrap()),
        "done: Read a\ndone (no activity for 600s) - 1 tool call"
    );
    // A zero cap disables it.
    let mut folds = Folds::new(Duration::ZERO).with_idle(secs(20));
    folds.set_text_visible(false);
    folds.launch_background("s", "P", &json!({"toolCallId": "P"}), None, t0);
    assert!(folds.poll(t0 + secs(100_000)).is_empty());
}

#[test]
fn zero_idle_disables_silence_finishes() {
    let t0 = Instant::now();
    let mut folds = Folds::new(Duration::ZERO).with_max_quiet(secs(600));
    folds.launch_background("s", "P", &json!({"toolCallId": "P"}), None, t0);
    folds.child_update("s", "P", &text_chunk("done"), t0 + secs(1));
    assert!(folds.poll(t0 + secs(100_000)).is_empty());
}

// Markers.

#[test]
fn a_marker_prefers_a_text_ended_candidate_and_skips_open_tools() {
    let t0 = Instant::now();
    let mut folds = Folds::new(Duration::ZERO);
    for id in ["A", "B", "C"] {
        folds.launch_background("s", id, &json!({"toolCallId": id}), None, t0);
    }
    // A: tool open. B: just finished a tool. C: ended with text, active
    // more recently than B.
    folds.child_update("s", "A", &tool_call("a1", "Write"), t0 + secs(1));
    folds.child_update("s", "B", &tool_call("b1", "Read"), t0 + secs(2));
    folds.child_update("s", "B", &tool_done("b1"), t0 + secs(2));
    folds.child_update("s", "C", &tool_call("c1", "Read"), t0 + secs(3));
    folds.child_update("s", "C", &tool_done("c1"), t0 + secs(3));
    folds.child_update("s", "C", &text_chunk("Report."), t0 + secs(4));
    folds.note_notification("s", t0 + secs(10));
    let done = folds.poll(t0 + secs(10));
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].parent, "C");
    assert!(done[0].confirmed);
    // The next marker falls back to B (no open tool), never to A.
    folds.note_notification("s", t0 + secs(11));
    let done = folds.poll(t0 + secs(11));
    assert_eq!(done[0].parent, "B");
    // With only A (tool open) left, a marker finishes nothing.
    folds.note_notification("s", t0 + secs(12));
    assert!(folds.poll(t0 + secs(12)).is_empty());
    assert!(
        folds
            .take_notes()
            .iter()
            .any(|n| n.contains("finished nothing"))
    );
    assert_eq!(folds.running_background("s"), vec!["A".to_owned()]);
}

#[test]
fn a_marker_confirms_a_provisionally_finished_card_without_a_new_card_update() {
    let t0 = Instant::now();
    let mut folds = watched(t0);
    folds.child_update("s", "P", &text_chunk("Report."), t0 + secs(1));
    let done = folds.poll(t0 + secs(30));
    assert!(!done[0].confirmed);
    folds.note_notification("s", t0 + secs(31));
    let done = folds.poll(t0 + secs(31));
    assert_eq!(reasons(&done), vec![("notification", true)]);
    assert!(done[0].update.is_none() && done[0].entry.is_none());
    assert!(!folds.live_background("s").contains(&"P".to_owned()));
}

// Prompt responses.

#[test]
fn a_prompt_response_finishes_the_cards_that_prompt_launched() {
    let t0 = Instant::now();
    let mut folds = Folds::new(Duration::ZERO).with_finish_entry(true);
    folds.launch_background("s", "A", &json!({"toolCallId": "A"}), Some("1"), t0);
    folds.launch_background("s", "B", &json!({"toolCallId": "B"}), Some("2"), t0);
    folds.child_update("s", "A", &tool_call("a1", "Write"), t0 + secs(1));
    let done = folds.finish_launched_by("s", "1", false, t0 + secs(5));
    assert_eq!(reasons(&done), vec![("prompt response", true)]);
    assert_eq!(done[0].parent, "A", "even with a tool open");
    assert_eq!(folds.running_background("s"), vec!["B".to_owned()]);
    // Handoff: B belongs to prompt 3.
    folds.reassign_launched_by("s", "2", "3");
    assert!(
        folds
            .finish_launched_by("s", "2", false, t0 + secs(6))
            .is_empty()
    );
    assert_eq!(
        folds.finish_launched_by("s", "3", true, t0 + secs(7)).len(),
        1
    );
}

// Entry, reopen, retire.

#[test]
fn one_entry_id_per_card_updated_across_reopen_and_refinish() {
    let t0 = Instant::now();
    let mut folds = watched(t0);
    folds.child_update("s", "P", &text_chunk("First."), t0 + secs(1));
    let done = folds.poll(t0 + secs(30));
    let Some(EntryOut::Append(first)) = &done[0].entry else {
        panic!("first finish appends");
    };
    assert_eq!(first["params"]["update"]["toolCallId"], "pons-finished-P");
    assert_eq!(first["params"]["update"]["sessionUpdate"], "tool_call");
    // Before the entry went out, a reopen sends no resume update.
    let out = folds.child_update("s", "P", &tool_call("c1", "Read"), t0 + secs(31));
    assert_eq!(out.len(), 1, "card only: {out:?}");
    folds.child_update("s", "P", &tool_done("c1"), t0 + secs(32));
    folds.child_update("s", "P", &text_chunk("Second."), t0 + secs(33));
    let done = folds.poll(t0 + secs(60));
    assert!(matches!(done[0].entry, Some(EntryOut::Append(_))));
    folds.mark_entry_sent("P");
    // Reopened after the entry was sent: the same entry resumes.
    let out = folds.child_update("s", "P", &tool_call("c2", "Read"), t0 + secs(61));
    assert_eq!(out.len(), 2, "card, then entry: {out:?}");
    let resumed = &out[1]["params"]["update"];
    assert_eq!(resumed["toolCallId"], "pons-finished-P");
    assert_eq!(resumed["sessionUpdate"], "tool_call_update");
    assert_eq!(resumed["status"], "in_progress");
    assert_eq!(resumed["content"][0]["content"]["text"], "resumed");
    folds.child_update("s", "P", &tool_done("c2"), t0 + secs(62));
    folds.child_update("s", "P", &text_chunk("Final answer."), t0 + secs(63));
    let done = folds.poll(t0 + secs(90));
    let Some(EntryOut::Update(last)) = &done[0].entry else {
        panic!("a later finish updates the entry");
    };
    let u = &last["params"]["update"];
    assert_eq!(u["toolCallId"], "pons-finished-P");
    assert_eq!(u["sessionUpdate"], "tool_call_update");
    assert_eq!(u["status"], "completed");
    assert_eq!(
        u["content"][0]["content"]["text"],
        "2 tool calls\nFinal answer."
    );
}

#[test]
fn provisional_cards_are_hidden_not_retired_and_resume_with_their_log() {
    let t0 = Instant::now();
    let mut folds = watched(t0);
    folds.child_update("s", "P", &tool_call("c1", "Read a"), t0 + secs(1));
    folds.child_update("s", "P", &tool_done("c1"), t0 + secs(1));
    folds.child_update("s", "P", &text_chunk("Thinking out loud."), t0 + secs(2));
    folds.poll(t0 + secs(30));
    assert!(folds.live_background("s").contains(&"P".to_owned()));
    // Turn end and a minute later: hidden from the roster, log kept.
    assert_eq!(folds.retire("s", Some(t0 + secs(40))).len(), 1);
    assert!(folds.roster("s", t0 + secs(41)).is_empty());
    assert!(!folds.live_background("s").contains(&"P".to_owned()));
    assert!(folds.retire_expired(t0 + secs(200)).is_empty());
    // Active again: still the background card, log intact, back in the roster.
    let out = folds.child_update("s", "P", &tool_call("c2", "Read b"), t0 + secs(300));
    let m = &out[0];
    assert_eq!(m["params"]["update"]["status"], "in_progress");
    assert!(log_text(m).starts_with("done: Read a\nrunning: Read b"));
    assert_eq!(folds.roster("s", t0 + secs(300)).len(), 1);
}

#[test]
fn a_retired_confirmed_card_leaves_a_tombstone_that_keeps_the_entry_id() {
    let t0 = Instant::now();
    let mut folds = watched(t0);
    folds.note_notification("s", t0 + secs(3));
    let done = folds.poll(t0 + secs(3));
    assert!(done[0].confirmed);
    folds.mark_entry_sent("P");
    assert_eq!(folds.retire("s", Some(t0 + secs(5))).len(), 1);
    // Activity much later recreates a background card, and the sent entry
    // resumes instead of a second one appearing.
    let out = folds.child_update("s", "P", &tool_call("c1", "Read"), t0 + secs(100));
    assert_eq!(out[0]["params"]["update"]["status"], "in_progress");
    assert!(
        out[0]["params"]["update"]["title"]
            .as_str()
            .unwrap()
            .starts_with("Review the diff: running")
    );
    assert_eq!(out[1]["params"]["update"]["toolCallId"], "pons-finished-P");
    folds.child_update("s", "P", &tool_done("c1"), t0 + secs(100));
    folds.child_update("s", "P", &text_chunk("Again."), t0 + secs(101));
    folds.note_notification("s", t0 + secs(102));
    let done = folds.poll(t0 + secs(102));
    assert!(matches!(done[0].entry, Some(EntryOut::Update(_))));
}

#[test]
fn tombstones_are_bounded() {
    let t0 = Instant::now();
    let mut folds = Folds::new(Duration::ZERO);
    for i in 0..(MAX_TOMBSTONES + 10) {
        let id = format!("P{i}");
        folds.launch_background("s", &id, &json!({"toolCallId": id}), None, t0);
        folds.note_notification("s", t0);
        folds.poll(t0);
        folds.retire("s", Some(t0 + secs(1)));
    }
    assert_eq!(folds.tombstones.len(), MAX_TOMBSTONES);
    assert_eq!(folds.tomb_order.len(), MAX_TOMBSTONES);
}

#[test]
fn finishing_a_removed_entry_is_a_noted_no_op() {
    let t0 = Instant::now();
    let mut folds = launched(t0);
    folds.child_update("s", "P", &tool_call("c1", "Read a"), t0);
    folds.forget_session("s");
    assert!(
        folds
            .finish_background("P", false, None, "stop", true, t0)
            .is_none()
    );
    assert!(folds.take_notes().iter().any(|n| n.contains("is gone")));
}

#[test]
fn a_later_fuller_input_replaces_the_stored_one() {
    let mut e = FoldEntry::new("s", "P");
    e.note_input(&json!({"rawInput": {"subagent_type": "wor"}}));
    e.note_input(&json!({"rawInput": {"subagent_type": "worker", "description": "Fix it"}}));
    assert_eq!(e.roster_name(), "worker - Fix it");
}

#[test]
fn an_empty_or_missing_input_keeps_the_stored_one() {
    let mut e = FoldEntry::new("s", "P");
    e.note_input(&json!({"rawInput": {"subagent_type": "worker", "description": "Fix it"}}));
    e.note_input(&json!({"rawInput": {}}));
    e.note_input(&json!({"toolCallId": "P"}));
    assert_eq!(e.roster_name(), "worker - Fix it");
}
