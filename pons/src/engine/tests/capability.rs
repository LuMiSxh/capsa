// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn everything_pons_emits_is_ascii() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        notices: NoticeMode::Always,
        ..test_config()
    });
    let mut all = notify_a(&mut e, t0);
    e.on_agent(&chunk("B is working, and so on"), t0 + secs(11));
    all.extend(e.on_agent(
        &update(json!({"sessionUpdate": "plan", "entries": [
            {"content": "todo", "priority": "high", "status": "pending"}]})),
        t0 + secs(11),
    ));
    all.extend(e.tick(t0 + secs(12)));
    all.extend(e.on_agent(&chunk(&"y".repeat(500)), t0 + secs(12)));
    all.extend(e.on_agent(
        &update(json!({"_meta": {"claudeCode": {"parentToolUseId": "B"}},
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "z".repeat(500)}})),
        t0 + secs(13),
    ));
    all.extend(e.tick(t0 + secs(14)));
    // B goes idle, then a real Stop on a third subagent.
    all.extend(e.tick(t0 + secs(30)));
    all.extend(e.on_agent(&response(1), t0 + secs(31)));
    e.on_client(&prompt(2), t0 + secs(32));
    e.on_agent(&real_parent("C"), t0 + secs(32));
    e.on_agent(&real_launch_result("C"), t0 + secs(32));
    all.extend(e.on_client(&cancel(), t0 + secs(33)));
    e.on_client(&prompt(3), t0 + secs(33));
    all.extend(e.on_client(&cancel(), t0 + secs(34)));
    all.extend(e.tick(t0 + secs(36)));
    let synthesized: Vec<_> = all
        .iter()
        .filter_map(|a| match a {
            Action::ToClient {
                line,
                synthesized: true,
            } => Some(line),
            _ => None,
        })
        .collect();
    assert!(
        synthesized.len() >= 10,
        "scenario covers many shapes: {}",
        synthesized.len()
    );
    for line in &synthesized {
        let text = String::from_utf8_lossy(line);
        assert!(text.is_ascii(), "non-ASCII on the wire: {text}");
    }
    // The shapes this guards are actually in the scenario.
    let kinds: HashSet<String> = synthesized
        .iter()
        .filter_map(|l| serde_json::from_slice::<Value>(l).ok())
        .filter_map(|m| {
            m["params"]["update"]["sessionUpdate"]
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    for kind in ["tool_call", "tool_call_update", "plan", "notice"] {
        assert!(kinds.contains(kind), "{kind} missing from the scenario");
    }
}

#[test]
fn steps_aside_when_the_client_renders_subagents_natively() {
    let t0 = Instant::now();
    let mut e = engine();
    let out = e.on_client(&initialize_with(json!({"subagents": {}})), t0);
    assert_eq!(notes(&out).len(), 1);
    assert!(notes(&out)[0].contains("stepping aside"));
    assert!(
        out.iter().any(|a| matches!(a, Action::ToAgent(_))),
        "initialize still reaches the agent"
    );
    e.on_client(&prompt(1), t0);
    for raw in [
        real_parent("A"),
        real_launch_result("A"),
        kid("A", "a1", "Read a"),
        task_notification(),
        line(
            json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s",
            "update": {"sessionUpdate": "plan", "entries": []}}}),
        ),
    ] {
        assert_untouched(&e.on_agent(&raw, t0 + secs(1)), &raw);
    }
    // The cancel filter's subagent assumptions are off too.
    let out = e.on_client(&cancel(), t0 + secs(10));
    assert!(out.iter().any(|a| matches!(a, Action::ToAgent(_))));
    assert!(
        !out.iter()
            .any(|a| matches!(a, Action::ToClient { .. } | Action::Note(_)))
    );
    // Nothing is finished by silence, no roster, no notice, no entry.
    assert!(e.tick(t0 + secs(120)).is_empty());
    let out = e.on_agent(&response(1), t0 + secs(121));
    assert_eq!(sent_to_client(&out).len(), 1);
}

#[test]
fn force_features_keeps_pons_on_with_native_subagents() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        force_features: true,
        ..test_config()
    });
    let out = e.on_client(&initialize_with(json!({"subagents": {}})), t0);
    assert!(notes(&out)[0].contains("--force-features"));
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    let out = e.on_agent(&real_launch_result("A"), t0 + secs(1));
    assert_eq!(
        updates_for(&out, "A")[0]["params"]["update"]["status"],
        "in_progress"
    );
}

#[test]
fn absent_null_or_false_subagent_capability_does_not_step_aside() {
    for caps in [
        json!({}),
        json!({"subagents": null}),
        json!({"subagents": false}),
    ] {
        let mut e = engine();
        let out = e.on_client(&initialize_with(caps), Instant::now());
        assert!(!notes(&out).iter().any(|n| n.contains("stepping aside")));
        let t0 = Instant::now();
        e.on_client(&prompt(1), t0);
        e.on_agent(&real_parent("A"), t0);
        let out = e.on_agent(&real_launch_result("A"), t0);
        assert_eq!(updates_for(&out, "A").len(), 1);
    }
}
