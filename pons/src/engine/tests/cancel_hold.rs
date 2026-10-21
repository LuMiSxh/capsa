// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_cancel_is_held_not_forwarded_and_the_prompt_is_answered_at_once() {
    let t0 = Instant::now();
    let mut e = five_background(t0, test_config());
    let out = e.on_client(&cancel(), t0 + secs(10));
    assert!(agent_msgs(&out).is_empty());
    assert!(has_note(&out, "cancel deferred"));
    let msgs = client_lines(&out);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["id"], 1);
    assert_eq!(msgs[0]["result"]["stopReason"], "cancelled");
    // Nothing is decided inside the window.
    assert!(agent_msgs(&e.tick(t0 + secs(10) + ms(399))).is_empty());
    assert_eq!(running(&e), 5);
}

#[test]
fn a_cancel_with_no_prompt_inside_the_window_is_a_stop_forwarded_once() {
    let t0 = Instant::now();
    let mut e = five_background(t0, test_config());
    let c = cancel();
    e.on_client(&c, t0 + secs(10));
    let out = e.tick(t0 + secs(10) + ms(400));
    assert_eq!(agent_msgs(&out).len(), 1);
    assert_eq!(agent_msgs(&out)[0]["method"], "session/cancel");
    assert!(has_note(&out, "stop: no prompt followed"));
    assert_eq!(running(&e), 0);
    let stopped = client_lines(&out)
        .iter()
        .filter(|m| m["params"]["update"]["status"] == "failed")
        .count();
    assert!(stopped >= 5, "{out:?}");
    let ids = entries(&out).len();
    assert_eq!(ids, 5);
    // Once only: later ticks decide nothing, and the adapter's answer to
    // the cancelled prompt is swallowed without a second round of cards.
    assert!(
        e.tick(t0 + secs(11))
            .iter()
            .all(|a| !matches!(a, Action::ToAgent(_)))
    );
    let late = e.on_agent(&response(1), t0 + secs(11));
    assert!(client_lines(&late).is_empty());
    assert!(entries(&late).is_empty());
    assert!(
        client_lines(&late)
            .iter()
            .all(|m| m["params"]["update"]["status"] != "failed")
    );
}

#[test]
fn a_second_cancel_inside_the_window_is_forwarded_at_once() {
    let t0 = Instant::now();
    let mut e = five_background(t0, test_config());
    e.on_client(&cancel(), t0 + secs(10));
    let c = cancel();
    let out = e.on_client(&c, t0 + secs(10) + ms(150));
    assert!(out.contains(&Action::ToAgent(c)));
    assert!(has_note(&out, "stop escape"));
    assert_eq!(running(&e), 0);
    // The window is closed: the tick sends no second cancel.
    assert!(
        e.tick(t0 + secs(11))
            .iter()
            .all(|a| !matches!(a, Action::ToAgent(_)))
    );
}

#[test]
fn zero_stop_window_keeps_holding_a_second_cancel() {
    let t0 = Instant::now();
    let mut e = five_background(
        t0,
        Config {
            stop_window: Duration::ZERO,
            ..test_config()
        },
    );
    e.on_client(&cancel(), t0 + secs(10));
    let out = e.on_client(&cancel(), t0 + secs(10) + ms(150));
    assert!(agent_msgs(&out).is_empty());
    let out = e.tick(t0 + secs(11));
    assert_eq!(agent_msgs(&out).len(), 1);
}

#[test]
fn without_subagents_the_cancel_is_forwarded_at_once() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&chunk("busy"), t0 + ms(10));
    let c = cancel();
    assert_eq!(
        e.on_client(&c, t0 + ms(20)),
        vec![Action::ToAgent(c.clone())]
    );
    assert!(e.tick(t0 + secs(5)).is_empty());
}

#[test]
fn cancel_mode_forwards_the_cancel_like_before() {
    let t0 = Instant::now();
    let mut e = five_background(
        t0,
        Config {
            interrupt: Some(InterruptMode::Cancel),
            ..test_config()
        },
    );
    let c = cancel();
    assert_eq!(
        e.on_client(&c, t0 + secs(10)),
        vec![Action::ToAgent(c.clone())]
    );
}
