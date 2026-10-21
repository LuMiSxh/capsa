// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn replayed_history_registers_no_cards_and_is_never_closed() {
    for method in ["session/load", "session/resume"] {
        let t0 = Instant::now();
        let mut e = engine();
        e.on_client(&load_request(method), t0);
        for raw in replayed_history() {
            assert_untouched(&e.on_agent(&raw, t0), &raw);
        }
        let resp = line(json!({"jsonrpc": "2.0", "id": "L", "result": {}}));
        let out = e.on_agent(&resp, t0);
        assert!(notes(&out)[0].contains("replay of s done, 10 update(s)"));
        assert!(sent_to_client(&out).ends_with(&[(resp, false)]));
        // Long after: no idle-finish burst, no entries, notices or roster.
        for secs_later in [30, 60, 600] {
            assert!(e.tick(t0 + secs(secs_later)).is_empty());
        }
        // A cancel finds nothing in flight and goes through.
        e.on_client(&prompt(1), t0 + secs(700));
        let out = e.on_client(&cancel(), t0 + secs(710));
        assert!(out.iter().any(|a| matches!(a, Action::ToAgent(_))));
    }
}

#[test]
fn a_prompt_ends_the_replay_window_and_new_launches_are_tracked() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&load_request("session/load"), t0);
    // The prompt arrives before the load response: the replay is over.
    e.on_client(&prompt(1), t0 + secs(1));
    e.on_agent(&real_parent("N"), t0 + secs(1));
    let out = e.on_agent(&real_launch_result("N"), t0 + secs(2));
    assert_eq!(
        updates_for(&out, "N")[0]["params"]["update"]["status"],
        "in_progress"
    );
}

#[test]
fn live_work_after_the_load_response_is_tracked_again() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&load_request("session/load"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(
        &line(json!({"jsonrpc": "2.0", "id": "L", "result": {}})),
        t0,
    );
    e.on_client(&prompt(1), t0 + secs(5));
    e.on_agent(&real_parent("N"), t0 + secs(5));
    let out = e.on_agent(&real_launch_result("N"), t0 + secs(6));
    assert_eq!(updates_for(&out, "N").len(), 1);
    // Only the new subagent is on the books.
    let out = e.on_client(&cancel(), t0 + secs(20));
    assert!(notes(&out)[0].contains("cancel deferred"));
    assert!(notes(&out)[0].contains("1 subagent(s) in flight"));
}
