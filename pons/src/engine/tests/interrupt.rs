// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_replayed_interrupt_marker_is_dropped_between_other_messages() {
    let t0 = Instant::now();
    let mut e = engine();
    let out = replay_with_marker(&mut e, t0);
    assert_eq!(texts(&out), ["before", "after"]);
    // A split marker is held and dropped too; other text is not delayed.
    let out = e.on_agent(&chunk_of("user_message_chunk", "[Request interr"), t0);
    assert!(out.is_empty());
    let out = e.on_agent(
        &chunk_of("user_message_chunk", "upted by user for tool use]"),
        t0,
    );
    assert!(out.is_empty());
    let out = e.on_agent(&chunk_of("user_message_chunk", "hello"), t0);
    assert_eq!(texts(&out), ["hello"]);
    let resp = line(json!({"jsonrpc": "2.0", "id": "L", "result": {}}));
    let out = e.on_agent(&resp, t0);
    assert!(notes(&out)[0].contains("dropped 2 replayed interrupt marker(s)"));
}

#[test]
fn interrupt_text_outside_a_replay_or_with_the_flag_is_forwarded() {
    let t0 = Instant::now();
    // No replay window: live text is never dropped.
    let mut e = engine();
    let raw = chunk_of("user_message_chunk", INTERRUPT);
    assert_untouched(&e.on_agent(&raw, t0), &raw);
    // After the load response and after a prompt the window is closed.
    replay_with_marker(&mut e, t0);
    e.on_agent(
        &line(json!({"jsonrpc": "2.0", "id": "L", "result": {}})),
        t0,
    );
    assert_untouched(&e.on_agent(&raw, t0), &raw);
    // The flag restores the old behavior.
    let mut e = Engine::new(Config {
        drop_interrupt_markers: false,
        ..test_config()
    });
    let out = replay_with_marker(&mut e, t0);
    assert_eq!(texts(&out), ["before", INTERRUPT, "after"]);
    let resp = line(json!({"jsonrpc": "2.0", "id": "L", "result": {}}));
    assert!(!notes(&e.on_agent(&resp, t0))[0].contains("interrupt"));
}
