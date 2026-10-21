// SPDX-License-Identifier: MPL-2.0

use super::*;

/// A subagent that resumes long after its notification must not hand that
/// notification to another card.
#[test]
fn a_resumed_subagent_does_not_pass_its_notification_on() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid_ended("B", "b1", "Read b"), t0 + secs(2));
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(5));
    let out = e.on_agent(&task_notification(), t0 + secs(6));
    assert_eq!(
        updates_for(&out, "B")[0]["params"]["update"]["status"],
        "completed"
    );
    // B resumes a minute later: its own notification, not A's.
    e.on_agent(&kid_ended("B", "b2", "Read b2"), t0 + secs(66));
    let out = e.tick(t0 + secs(67));
    assert!(
        updates_for(&out, "A")
            .iter()
            .all(|m| m["params"]["update"]["status"] == "in_progress"),
        "A was finished by a notification that was B's"
    );
    let mut running = e.folds.running_background("s");
    running.sort();
    assert_eq!(running, ["A", "B"]);
}

/// toolu_01SJUd... of a d755d33e log: finished by the prompt response,
/// then active again through parentToolUseId, then finished by a
/// notification. One entry, updated in place.
#[test]
fn a_card_finished_by_the_prompt_response_reactivates_and_finishes_in_place() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(1));
    let out = e.on_agent(&response(1), t0 + secs(5));
    assert!(
        notes(&out)
            .iter()
            .any(|n| n.contains("finished (prompt response)"))
    );
    assert_eq!(entry_trail(&out), trail(&[("tool_call", "completed")]));

    let out = e.on_agent(&kid("A", "a2", "Read a2"), t0 + secs(10));
    assert!(
        notes(&out).iter().any(|n| n.contains("active again")),
        "{:?}",
        notes(&out)
    );
    assert_eq!(
        updates_for(&out, "A")[0]["params"]["update"]["status"],
        "in_progress"
    );
    assert_eq!(
        entry_trail(&out),
        trail(&[("tool_call_update", "in_progress")])
    );
    assert_eq!(e.folds.running_background("s"), ["A"]);
    assert!(e.sessions["s"].in_flight.contains("A"));

    e.on_agent(&kid_end("A", "a2"), t0 + secs(11));
    e.on_agent(&child_text("A", "Second run done."), t0 + secs(12));
    let out = e.on_agent(&task_notification(), t0 + secs(13));
    assert_eq!(
        entry_trail(&out),
        trail(&[("tool_call_update", "completed")])
    );
    assert!(e.folds.running_background("s").is_empty());
}

/// Claude Code notifies again when a subagent stops again; the repeats of
/// a finished subagent that did not resume change nothing.
#[test]
fn repeated_task_notifications_for_a_finished_card_add_nothing() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(1));
    e.on_agent(&child_text("A", "Done."), t0 + secs(2));
    let out = e.on_agent(&task_notification(), t0 + secs(3));
    assert_eq!(entry_trail(&out), trail(&[("tool_call", "completed")]));
    for at in [5, 20, 75] {
        let mut out = e.on_agent(&task_notification(), t0 + secs(at));
        out.extend(e.tick(t0 + secs(at + 1)));
        assert!(entries(&out).is_empty(), "{:?}", entries(&out));
        assert!(updates_for(&out, "A").is_empty());
        assert!(notes(&out).iter().all(|n| !n.contains("finished (")));
    }
    assert!(e.held.is_empty());
}

/// toolu_01Y1H8... of a d755d33e log: notification, retired, active again,
/// silent finish, then the notification of the second run. One entry id,
/// one append.
#[test]
fn every_finish_path_after_a_retirement_updates_the_same_entry() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(1));
    e.on_agent(&child_text("A", "Done."), t0 + secs(2));
    let mut all = e.on_agent(&task_notification(), t0 + secs(3));
    // The next turn begins and ends: the finished card is retired.
    all.extend(e.on_agent(&response(1), t0 + secs(4)));
    e.on_client(&prompt(2), t0 + secs(5));
    all.extend(e.on_agent(&response(2), t0 + secs(6)));
    all.extend(e.tick(t0 + secs(7)));
    // Active again, then silent, then notified.
    all.extend(e.on_agent(&kid_ended("A", "a2", "Read a2"), t0 + secs(100)));
    all.extend(e.on_agent(&child_text("A", "Again."), t0 + secs(101)));
    all.extend(e.tick(t0 + secs(130)));
    all.extend(e.on_agent(&task_notification(), t0 + secs(135)));
    all.extend(e.tick(t0 + secs(140)));
    let seen = entry_trail(&all);
    assert_eq!(
        seen.iter().filter(|(k, _)| k == "tool_call").count(),
        1,
        "{seen:?}"
    );
    assert_eq!(
        seen.last().unwrap(),
        &("tool_call_update".to_owned(), "completed".to_owned())
    );
    assert!(e.folds.running_background("s").is_empty());
}
