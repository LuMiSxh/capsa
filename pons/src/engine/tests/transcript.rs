// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn initialize_gets_the_subagent_transcript_capability() {
    let t0 = Instant::now();
    // _meta absent, _meta present, and clientCapabilities absent.
    let cases = [
        (
            json!({"session": {"notices": {}}}),
            json!({"session": {"notices": {}}, "_meta": {"subagent-transcript": true}}),
        ),
        (
            json!({"_meta": {"x": 1}, "fs": {}}),
            json!({"_meta": {"x": 1, "subagent-transcript": true}, "fs": {}}),
        ),
        (
            json!({"_meta": null}),
            json!({"_meta": {"subagent-transcript": true}}),
        ),
        (json!({}), json!({"_meta": {"subagent-transcript": true}})),
    ];
    for (caps, want) in cases {
        let mut e = engine();
        let out = e.on_client(&initialize_with(caps), t0);
        assert_eq!(notes(&out).len(), 1);
        assert!(notes(&out)[0].contains("subagent-transcript"));
        let sent = to_agent(&out);
        assert_eq!(sent["params"]["clientCapabilities"], want);
        assert_eq!(sent["params"]["protocolVersion"], 1);
        assert_eq!(sent["id"], 0);
        assert_eq!(sent["method"], "initialize");
    }
    let mut e = engine();
    let raw = line(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": 1}}));
    let sent = to_agent(&e.on_client(&raw, t0));
    assert_eq!(
        sent["params"]["clientCapabilities"]["_meta"]["subagent-transcript"],
        true
    );
}

#[test]
fn initialize_is_left_alone_when_the_capability_is_not_pons_to_add() {
    let t0 = Instant::now();
    let untouched = |mut e: Engine, caps: Value| {
        let raw = initialize_with(caps);
        let out = e.on_client(&raw, t0);
        assert!(out.contains(&Action::ToAgent(raw)), "{out:?}");
        assert!(
            !notes(&out)
                .iter()
                .any(|n| n.contains("subagent-transcript"))
        );
    };
    // The client renders subagents natively (aside), with and without force.
    untouched(engine(), json!({"subagents": {}}));
    let forced = Config {
        force_features: true,
        ..test_config()
    };
    untouched(Engine::new(forced), json!({"subagents": {}}));
    // The flag, and flat forwarding (--no-fold).
    let off = Config {
        subagent_text: false,
        ..test_config()
    };
    untouched(Engine::new(off), json!({"fs": {}}));
    let flat = Config {
        fold: false,
        ..test_config()
    };
    untouched(Engine::new(flat), json!({"fs": {}}));
    // Already set by the client, or explicitly refused.
    untouched(engine(), json!({"_meta": {"subagent-transcript": true}}));
    untouched(engine(), json!({"_meta": {"subagent-transcript": false}}));
    // Not an object that can hold it.
    untouched(engine(), json!({"_meta": 5}));
    untouched(engine(), json!("caps"));
}

#[test]
fn a_failing_initialize_is_forwarded_unchanged() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        inject_failure: Some("initialize".into()),
        ..test_config()
    });
    let raw = initialize_with(json!({"fs": {}}));
    let out = e.on_client(&raw, t0);
    assert!(notes(&out)[0].contains("fail-safe"));
    assert_eq!(out[1], Action::ToAgent(raw));
    assert_eq!(out.len(), 2);
}

#[test]
fn replayed_subagent_text_is_dropped_once_the_adapter_forwards_it() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&initialize_with(json!({})), t0);
    e.on_client(&load_request("session/load"), t0);
    let flat = child_text("A", "the subagent said this");
    assert!(e.on_agent(&flat, t0).is_empty());
    let thought = update(json!({"_meta": {"claudeCode": {"parentToolUseId": "A"}},
        "sessionUpdate": "agent_thought_chunk",
        "content": {"type": "text", "text": "hm"}}));
    assert!(e.on_agent(&thought, t0).is_empty());
    // Everything else in the history is untouched, child tool calls too.
    for raw in [
        chunk("main agent"),
        kid("A", "a1", "Read a"),
        chunk_of("user_message_chunk", "hi"),
    ] {
        assert_untouched(&e.on_agent(&raw, t0), &raw);
    }
    let resp = line(json!({"jsonrpc": "2.0", "id": "L", "result": {}}));
    let out = e.on_agent(&resp, t0);
    assert!(notes(&out)[0].contains("replay of s done, 5 update(s) seen"));
    assert!(notes(&out)[0].contains("dropped 2 replayed subagent text chunk(s)"));
    // Live subagent text after the load is folded, not dropped or flat.
    e.on_client(&prompt(1), t0 + secs(1));
    e.on_agent(&real_parent("N"), t0 + secs(1));
    e.on_agent(&kid("N", "n1", "Read n"), t0 + secs(1));
    let out = e.on_agent(&child_text("N", "live text"), t0 + secs(2));
    assert!(sent_to_client(&out).iter().all(|(_, synth)| *synth));
    assert!(card_texts(&out, "N")[0].ends_with("agent: live text"));
}

#[test]
fn replayed_subagent_text_is_untouched_when_pons_did_not_ask_for_it() {
    let t0 = Instant::now();
    let off = Config {
        subagent_text: false,
        ..test_config()
    };
    let mut e = Engine::new(off);
    e.on_client(&initialize_with(json!({})), t0);
    e.on_client(&load_request("session/load"), t0);
    let flat = child_text("A", "text");
    assert_untouched(&e.on_agent(&flat, t0), &flat);
}

#[test]
fn replayed_subagent_text_is_dropped_when_the_client_set_the_capability_itself() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(
        &initialize_with(json!({"_meta": {"subagent-transcript": true}})),
        t0,
    );
    e.on_client(&load_request("session/resume"), t0);
    assert!(e.on_agent(&child_text("A", "text"), t0).is_empty());
}

#[test]
fn the_final_message_is_accumulated_over_chunks_into_card_and_entry() {
    let t0 = Instant::now();
    let mut e = engine();
    // Text before the last tool call is not part of the result.
    text_subagent(&mut e, t0, &["searching"]);
    e.on_agent(&kid_ended("A", "a3", "Read c"), t0 + secs(2));
    e.on_agent(&child_text("A", "Found 3 "), t0 + secs(3));
    e.on_agent(
        &child_text("A", "issues.\n\n\n\n1. bad name\n"),
        t0 + secs(3),
    );
    e.on_agent(&child_text("A", "2. no test   \n"), t0 + secs(3));
    // Running: the tail of the current text on one line.
    let running = e.tick(t0 + secs(4));
    assert!(
        card_texts(&running, "A")
            .last()
            .unwrap()
            .ends_with("agent: Found 3 issues. 1. bad name 2. no test")
    );
    let out = e.tick(t0 + secs(30));
    let want = "Found 3 issues.\n\n1. bad name\n2. no test";
    let card = card_texts(&out, "A");
    assert_eq!(
        card.last().unwrap(),
        &format!(
            "done: Read a\ndone: Grep b\ndone: Read c\nagent: {want}\ndone (no activity for 20s) - 3 tool calls"
        )
    );
    let n = &entries(&out)[0]["params"]["update"];
    assert_eq!(n["toolCallId"], "pons-finished-A");
    assert_eq!(
        n["content"][0]["content"]["text"],
        format!("3 tool calls\n{want}")
    );
}

#[test]
fn the_final_message_keeps_its_head_up_to_2000_characters() {
    let t0 = Instant::now();
    let mut e = engine();
    let long = format!("HEAD {}", "x".repeat(3000));
    text_subagent(&mut e, t0, &[&long[..1500], &long[1500..]]);
    let out = e.tick(t0 + secs(30));
    let n = &entries(&out)[0]["params"]["update"];
    let text = n["content"][0]["content"]["text"].as_str().unwrap();
    let (calls, result) = text.split_once('\n').unwrap();
    assert_eq!(calls, "2 tool calls");
    assert_eq!(result.chars().count(), 2000);
    assert!(result.starts_with("HEAD xxx") && result.ends_with("xxx..."));
    assert!(text.is_ascii());
    // At the cap exactly nothing is cut.
    let mut e = engine();
    text_subagent(&mut e, t0, &[&"y".repeat(2000)]);
    let out = e.tick(t0 + secs(30));
    let text = entries(&out)[0]["params"]["update"]["content"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(text, format!("2 tool calls\n{}", "y".repeat(2000)));
}

#[test]
fn the_entry_has_only_the_count_when_the_subagent_said_nothing() {
    let t0 = Instant::now();
    let mut e = engine();
    text_subagent(&mut e, t0, &[]);
    // No text at all: only a marker can end it.
    let out = e.on_agent(&task_notification(), t0 + secs(30));
    let n = &entries(&out)[0]["params"]["update"];
    assert_eq!(n["content"][0]["content"]["text"], "2 tool calls");
}
