// SPDX-License-Identifier: MPL-2.0

use super::*;

// Shapes below are taken from a real Zed session (adapter
// claude-agent-acp, two background `Agent` calls).

#[test]
fn real_parent_shapes_are_detected_by_name_hint() {
    let by_meta = json!({"_meta": {"claudeCode": {"toolName": "Agent"}}});
    let by_name = json!({"name": "Agent"});
    let by_title = json!({"title": "Task", "kind": "think"});
    let neither =
        json!({"title": "Task", "kind": "other", "_meta": {"claudeCode": {"toolName": "Bash"}}});
    assert!(is_subagent_tool(&by_meta));
    assert!(is_subagent_tool(&by_name));
    assert!(is_subagent_tool(&by_title));
    assert!(!is_subagent_tool(&neither));
}

#[test]
fn tool_call_named_by_a_child_becomes_a_parent_whatever_its_name() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    // No name hint at all: a generic tool call.
    e.on_agent(
        &update(json!({"sessionUpdate": "tool_call", "toolCallId": "X", "title": "Delegate", "kind": "other"})),
        t0,
    );
    let out = e.on_agent(&kid("X", "k1", "Read a"), t0 + secs(1));
    assert!(matches!(&out[0], Action::Note(n) if n.contains("became a subagent parent")));
    assert_eq!(updates_for(&out, "X").len(), 1, "folded into X");
    // X's own updates are not main-agent activity, so the cancel is held.
    e.on_agent(
        &update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "X", "content": []})),
        t0 + secs(3),
    );
    let out = e.on_client(&cancel(), t0 + secs(4));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
}

#[test]
fn background_launch_keeps_the_card_running() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    let out = e.on_agent(&real_launch_result("A"), t0 + secs(1));
    let shown = updates_for(&out, "A");
    assert_eq!(shown.len(), 1);
    let u = &shown[0]["params"]["update"];
    assert_eq!(u["status"], "in_progress");
    assert!(u["rawOutput"].is_null(), "launch blob is not shown");
    assert_eq!(card_text(&shown[0]), "running in the background");
    assert_eq!(u["title"], "Agent: running 0s");
    assert!(!out.iter().any(|a| matches!(
        a,
        Action::ToClient {
            synthesized: false,
            ..
        }
    )));
    // Even before any child update the launched subagent holds the cancel.
    let out = e.on_client(&cancel(), t0 + secs(5));
    assert!(!out.iter().any(|a| matches!(a, Action::ToAgent(_))));
}

#[test]
fn sync_agent_completion_is_forwarded_untouched() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    let done = update(
        json!({"toolCallId": "A", "sessionUpdate": "tool_call_update",
        "status": "completed", "rawOutput": [{"type": "text", "text": "Found 3 files."}]}),
    );
    let out = e.on_agent(&done, t0 + secs(1));
    assert_eq!(
        out,
        vec![Action::ToClient {
            line: done,
            synthesized: false
        }]
    );
}

#[test]
fn notification_finishes_the_subagent_that_went_quiet() {
    let t0 = Instant::now();
    let mut e = two_background_with(
        t0,
        Config {
            notify_settle: secs(2),
            ..test_config()
        },
    );
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(2));
    e.on_agent(&kid_ended("B", "b1", "Read b"), t0 + secs(2));
    e.on_agent(&kid_ended("A", "a2", "Read a2"), t0 + secs(10));
    e.on_agent(&kid_ended("B", "b2", "Read b2"), t0 + secs(10));
    // A ends; its notification arrives, B reads on after it.
    e.on_agent(&task_notification(), t0 + secs(12));
    e.on_agent(&kid_ended("B", "b3", "Read b3"), t0 + secs(13));
    assert!(
        updates_for(&e.tick(t0 + secs(13)), "A")
            .iter()
            .all(|m| m["params"]["update"]["status"] == "in_progress"),
        "still settling"
    );

    let out = e.tick(t0 + secs(15));
    let a = updates_for(&out, "A");
    assert_eq!(a.len(), 1);
    assert_eq!(a[0]["params"]["update"]["status"], "completed");
    assert_eq!(
        card_text(&a[0]),
        "done: Read a\ndone: Read a2\ndone - 2 tool calls"
    );
    // The announcement follows the parent's final update, same flush.
    let lines = client_lines(&out);
    let at = lines
        .iter()
        .position(|m| m["params"]["update"]["toolCallId"] == "A")
        .unwrap();
    let n = &lines[at + 1]["params"]["update"];
    assert_eq!(n["sessionUpdate"], "tool_call");
    assert_eq!(n["toolCallId"], "pons-finished-A");
    assert_eq!(n["title"], "Agent finished");
    assert_eq!(n["kind"], "other");
    assert_eq!(n["status"], "completed");
    assert_eq!(
        n["content"],
        json!([{"type": "content", "content": {"type": "text", "text": "2 tool calls"}}])
    );
    assert_eq!(entries(&out).len(), 1, "only A is announced");
    // The entry is not main-agent activity and not a parent.
    assert!(!e.sessions["s"].parents.contains("pons-finished-A"));
    assert!(
        updates_for(&out, "B")
            .iter()
            .all(|m| m["params"]["update"]["status"] == "in_progress")
    );
    assert!(
        out.iter()
            .any(|a| matches!(a, Action::Note(n) if n.contains("A finished (notification)")))
    );
    // B is still held in flight; A no longer is.
    let s = &e.sessions["s"];
    assert!(s.in_flight.contains("B") && !s.in_flight.contains("A"));
}

#[test]
fn wrongly_finished_card_reopens_and_the_notification_is_rematched() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    // B went quiet first (a long think), A is the one that ends.
    e.on_agent(&kid_ended("B", "b1", "Read b"), t0 + secs(2));
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(5));
    let out = e.on_agent(&task_notification(), t0 + secs(6));
    assert_eq!(
        updates_for(&out, "B")[0]["params"]["update"]["status"],
        "completed"
    );

    // B was alive after all.
    let out = e.on_agent(&kid_ended("B", "b2", "Read b2"), t0 + secs(10));
    let b = updates_for(&out, "B");
    assert_eq!(b[0]["params"]["update"]["status"], "in_progress");
    assert!(!card_text(&b[0]).contains("done -"));
    let out = e.tick(t0 + secs(11));
    let a = updates_for(&out, "A");
    assert_eq!(a[0]["params"]["update"]["status"], "completed");
    assert!(
        updates_for(&out, "B")
            .iter()
            .all(|m| m["params"]["update"]["status"] == "in_progress")
    );
}

#[test]
fn notification_without_background_subagent_is_ignored() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&task_notification(), t0 + secs(1));
    assert!(e.tick(t0 + secs(10)).is_empty());
}

#[test]
fn idle_heuristic_finishes_after_final_text_with_nothing_open() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(2));
    e.on_agent(&child_text("A", "All clear."), t0 + secs(2));
    assert!(
        updates_for(&e.tick(t0 + secs(21)), "A")
            .iter()
            .all(|m| m["params"]["update"]["status"] == "in_progress")
    );
    let out = e.tick(t0 + secs(22));
    let a = updates_for(&out, "A");
    assert_eq!(a[0]["params"]["update"]["status"], "completed");
    assert_eq!(
        card_text(&a[0]),
        "done: Read a\nagent: All clear.\ndone (no activity for 20s) - 1 tool call"
    );
    // Provisional: logged as such, no notice, the cancel filter stays armed.
    assert!(
        out.iter()
            .any(|x| matches!(x, Action::Note(n) if n.contains("finished (idle, provisional)")))
    );
    assert!(e.sessions["s"].in_flight.contains("A"));
    assert!(e.tick(t0 + secs(60)).is_empty(), "finished once");
}

#[test]
fn no_idle_finish_while_a_child_tool_is_open() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    // A long-running tool, final text before it: still working.
    e.on_agent(&child_text("A", "Running the big job."), t0 + secs(1));
    e.on_agent(&kid("A", "a1", "Bash: cargo test"), t0 + secs(2));
    let out = e.tick(t0 + secs(500));
    assert!(entries(&out).is_empty());
    let a = updates_for(&out, "A");
    assert!(
        a.iter()
            .all(|m| m["params"]["update"]["status"] == "in_progress")
    );
    // The wait is visible, as a title-only update.
    let title = a[0]["params"]["update"]["title"].as_str().unwrap();
    assert!(title.ends_with("quiet 8m 15s"), "{title}");
    assert!(a[0]["params"]["update"].get("content").is_none());
    assert!(e.folds.running_background("s").contains(&"A".to_owned()));
}

/// Replays the false finishes from the real log (card
/// toolu_01McEgUjs87WRP1WBnAbtimq): the last child event is a tool
/// completion, then thinking or generating without a sign of life.
#[test]
fn no_idle_finish_after_a_tool_result() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid("A", "a1", "Read a"), t0 + secs(5));
    e.on_agent(&kid_end("A", "a1"), t0 + secs(6));
    for at in [30, 31, 50, 64] {
        let out = e.tick(t0 + secs(at));
        assert!(entries(&out).is_empty(), "at {at}");
        assert!(
            updates_for(&out, "A")
                .iter()
                .all(|m| m["params"]["update"]["status"] == "in_progress"),
            "at {at}"
        );
    }
    assert!(!e.folds.is_finished("A"));
}

#[test]
fn tool_result_then_silence_under_the_long_idle_is_not_finished() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid("A", "a1", "Read a"), t0 + secs(5));
    e.on_agent(&kid_end("A", "a1"), t0 + secs(6));
    let out = e.tick(t0 + secs(65));
    assert!(entries(&out).is_empty());
    assert!(!e.folds.is_finished("A"));
    assert!(e.folds.running_background("s").contains(&"A".to_owned()));
}

#[test]
fn tool_result_then_silence_past_the_long_idle_finishes_provisionally() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid("A", "a1", "Read a"), t0 + secs(5));
    e.on_agent(&kid_end("A", "a1"), t0 + secs(6));
    let out = e.tick(t0 + secs(67));
    let a = updates_for(&out, "A");
    assert_eq!(a[0]["params"]["update"]["status"], "completed");
    assert!(
        out.iter()
            .any(|x| matches!(x, Action::Note(n) if n.contains("finished (idle, provisional)")))
    );
    assert!(e.folds.is_finished("A"));
    assert!(e.sessions["s"].in_flight.contains("A"));
    assert!(e.tick(t0 + secs(90)).is_empty(), "heartbeat stops");
}

#[test]
fn without_subagent_text_only_the_long_cap_finishes() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        subagent_text: false,
        subagent_max_quiet: secs(600),
        ..test_config()
    });
    e.on_client(&initialize(false), t0);
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(2));
    assert!(!e.tick(t0 + secs(300)).iter().any(is_finish_note));
    assert!(!e.tick(t0 + secs(601)).iter().any(is_finish_note));
    let out = e.tick(t0 + secs(602));
    assert!(
        out.iter().any(
            |x| matches!(x, Action::Note(n) if n.contains("finished (quiet-cap, provisional)"))
        )
    );
    // The notice is for confirmed finishes only.
    assert!(notices_in(&out).is_empty());
}

#[test]
fn a_monitor_as_the_last_call_keeps_the_card_open_until_its_timeout() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&child_text("A", "Watching the log."), t0 + secs(1));
    e.on_agent(
        &monitor("A", "m1", json!({"timeout_ms": 300_000})),
        t0 + secs(2),
    );
    e.on_agent(&kid_end("A", "m1"), t0 + secs(3));
    // Past the 60 s idle wait after a tool result, and the card still runs.
    for at in [60, 100, 331] {
        let out = e.tick(t0 + secs(at));
        assert!(!updates_for(&out, "A").iter().any(is_card_done), "at {at}");
    }
    assert!(e.folds.running_background("s").contains(&"A".to_owned()));
    let out = e.tick(t0 + secs(332));
    assert!(updates_for(&out, "A").iter().any(is_card_done));
}

#[test]
fn a_monitor_keeps_a_silent_subagent_open_past_the_quiet_cap() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        subagent_text: false,
        subagent_max_quiet: secs(600),
        ..test_config()
    });
    e.on_client(&initialize(false), t0);
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(
        &monitor("A", "m1", json!({"timeout_ms": 1_000_000})),
        t0 + secs(2),
    );
    e.on_agent(&kid_end("A", "m1"), t0 + secs(3));
    assert!(!e.tick(t0 + secs(1_031)).iter().any(is_finish_note));
    let out = e.tick(t0 + secs(1_032));
    assert!(
        out.iter().any(
            |x| matches!(x, Action::Note(n) if n.contains("finished (quiet-cap, provisional)"))
        )
    );
}

#[test]
fn a_monitor_without_a_timeout_keeps_the_card_open_for_thirty_minutes() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&monitor("A", "m1", json!({})), t0 + secs(2));
    e.on_agent(&kid_end("A", "m1"), t0 + secs(3));
    assert!(
        !updates_for(&e.tick(t0 + secs(1_831)), "A")
            .iter()
            .any(is_card_done)
    );
    assert!(
        updates_for(&e.tick(t0 + secs(1_832)), "A")
            .iter()
            .any(is_card_done)
    );
}

#[test]
fn a_marker_ends_a_card_with_a_running_monitor_only_when_no_other_qualifies() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(
        &monitor("A", "m1", json!({"timeout_ms": 300_000})),
        t0 + secs(2),
    );
    e.on_agent(&kid_end("A", "m1"), t0 + secs(2));
    // B qualifies, so the first notification ends B and A keeps its monitor.
    e.on_agent(&task_notification(), t0 + secs(3));
    assert!(e.folds.is_finished("B"));
    assert!(!e.folds.is_finished("A"));
    // Only A is left: the next notification ends it.
    e.on_agent(&task_notification(), t0 + secs(4));
    assert!(e.folds.is_finished("A"));
}

#[test]
fn a_stop_ends_a_card_whose_monitor_is_still_running() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(
        &monitor("A", "m1", json!({"timeout_ms": 300_000})),
        t0 + secs(2),
    );
    e.on_agent(&kid_end("A", "m1"), t0 + secs(2));
    e.on_client(&cancel(), t0 + secs(5));
    let out = e.on_client(&cancel(), t0 + secs(6));
    assert!(
        updates_for(&out, "A")
            .iter()
            .any(|m| m["params"]["update"]["status"] == "failed")
    );
    assert!(e.folds.running_background("s").is_empty());
}
