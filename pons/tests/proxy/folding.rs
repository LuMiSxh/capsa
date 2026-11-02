// SPDX-License-Identifier: MPL-2.0

use serde_json::{Value, json};

use super::background::{background_card, is_chunk, is_finish_entry};
use super::harness::{Harness, folded_text, has_flat_child_update, temp_path};
use super::initialize::initialize;

#[tokio::test]
async fn native_subagent_support_makes_pons_step_aside() {
    let log = temp_path("aside.log");
    let _ = std::fs::remove_file(&log);
    let mut h = Harness::start(&["--log", log.to_str().unwrap()]);
    h.send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "echo/initialize",
        "params": {"clientCapabilities": {"subagents": {}}}}),
    )
    .await;
    h.recv().await;
    // The real initialize request, with the capability Zed advertises.
    h.send(json!({"jsonrpc": "2.0", "id": 2, "method": "initialize",
        "params": {"protocolVersion": 1, "clientCapabilities": {"subagents": {}}}}))
        .await;
    h.prompt(3, "subagent").await;
    let seen = h
        .recv_until(|m| {
            m["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
                && has_flat_child_update(m)
        })
        .await;
    // Child updates arrive flat and the parent is untouched.
    assert_eq!(seen.iter().filter(|m| has_flat_child_update(m)).count(), 3);
    assert!(seen.iter().all(|m| folded_text(m).is_none()), "{seen:?}");
    // Info lines are buffered: pons writes them out when it exits.
    drop(h.stdin);
    tokio::time::timeout(std::time::Duration::from_secs(10), h.child.wait())
        .await
        .expect("pons exits")
        .unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    let _ = std::fs::remove_file(&log);
    assert_eq!(text.matches("stepping aside").count(), 1, "{text}");
}

#[tokio::test]
async fn folds_subagent_updates_into_one_parent_update() {
    let mut h = Harness::start(&[]);
    let seen = h.start_subagent_turn().await;
    assert!(
        !seen
            .iter()
            .any(|m| m["params"]["update"]["_meta"]["claudeCode"]["parentToolUseId"].is_string()),
        "no flat subagent update reaches the client: {seen:?}"
    );
    let text = folded_text(seen.last().unwrap()).unwrap();
    assert_eq!(text, "done: Read src/a.rs\nagent: reviewing the file");
    // The parent's own tool call was forwarded.
    assert!(
        seen.iter()
            .any(|m| m["params"]["update"]["sessionUpdate"] == "tool_call")
    );
}

#[tokio::test]
async fn replayed_subagent_text_does_not_flood_the_chat_on_load() {
    let load = |id: i64| {
        json!({"jsonrpc": "2.0", "id": id, "method": "session/load",
            "params": {"sessionId": "s1", "cwd": "/x", "mcpServers": []}})
    };
    let mut h = Harness::start(&[]);
    initialize(&mut h, 1, json!({})).await;
    h.send(load(2)).await;
    let seen = h.recv_until(|m| m["id"] == 2).await;
    assert!(!seen.iter().any(has_flat_child_update), "{seen:?}");
    assert!(
        seen.iter().any(
            |m| is_chunk(m) && m["params"]["update"]["content"]["text"] == "replayed main text"
        ),
        "{seen:?}"
    );
    // Without the injection nothing is dropped: the adapter would not send it.
    let mut h = Harness::start(&["--no-subagent-text"]);
    initialize(&mut h, 1, json!({})).await;
    h.send(load(2)).await;
    let seen = h.recv_until(|m| m["id"] == 2).await;
    assert!(seen.iter().any(has_flat_child_update), "{seen:?}");
}

#[tokio::test]
async fn the_replayed_interrupt_marker_is_dropped_unless_kept() {
    let user_texts = |seen: &[Value]| -> Vec<String> {
        seen.iter()
            .filter(|m| m["params"]["update"]["sessionUpdate"] == "user_message_chunk")
            .filter_map(|m| m["params"]["update"]["content"]["text"].as_str())
            .map(str::to_owned)
            .collect()
    };
    let load = json!({"jsonrpc": "2.0", "id": 2, "method": "session/load",
        "params": {"sessionId": "s1", "cwd": "/x", "mcpServers": []}});
    let mut h = Harness::start(&[]);
    initialize(&mut h, 1, json!({})).await;
    h.send(load.clone()).await;
    let seen = h.recv_until(|m| m["id"] == 2).await;
    assert_eq!(user_texts(&seen), ["replayed user text"], "{seen:?}");
    assert!(
        seen.iter().any(
            |m| is_chunk(m) && m["params"]["update"]["content"]["text"] == "replayed main text"
        ),
        "{seen:?}"
    );
    let mut h = Harness::start(&["--keep-interrupt-markers"]);
    initialize(&mut h, 1, json!({})).await;
    h.send(load).await;
    let seen = h.recv_until(|m| m["id"] == 2).await;
    assert_eq!(
        user_texts(&seen),
        ["replayed user text", "[Request interrupted by user]"],
        "{seen:?}"
    );
}

#[tokio::test]
async fn the_folded_card_shows_the_subagents_text_never_flat() {
    let mut h = Harness::start(&[]);
    initialize(&mut h, 1, json!({})).await;
    h.prompt(2, "subagent-result").await;
    let seen = h
        .recv_until(|m| folded_text(m).is_some_and(|t| t.contains("all clear.")))
        .await;
    assert!(!seen.iter().any(has_flat_child_update), "{seen:?}");
    let text = folded_text(seen.last().unwrap()).unwrap();
    assert_eq!(
        text,
        "done: Read src/a.rs\nrunning: Grep x\nagent: Result: all clear."
    );
}

#[tokio::test]
async fn the_finished_entry_carries_the_subagents_final_message() {
    let mut h = Harness::start(&[]);
    initialize(&mut h, 1, json!({})).await;
    let seen = background_card(&mut h, "background-result").await;
    assert!(!seen.iter().any(has_flat_child_update), "{seen:?}");
    let card = folded_text_of(&seen, "toolu_bg");
    assert!(
        card.contains("agent: All clear.\ndone - 1 tool call"),
        "{card}"
    );
    let entry = seen.last().unwrap();
    assert!(is_finish_entry(entry), "{seen:?}");
    assert_eq!(
        entry["params"]["update"]["content"][0]["content"]["text"],
        "1 tool call\nAll clear."
    );
    assert!(entry.to_string().is_ascii());
}

/// Returns the text of the last card update for `id`.
fn folded_text_of(seen: &[Value], id: &str) -> String {
    seen.iter()
        .rev()
        .find(|m| {
            m["params"]["update"]["toolCallId"] == id && m["params"]["update"]["content"].is_array()
        })
        .and_then(|m| {
            m["params"]["update"]["content"].as_array()?.last()?["content"]["text"]
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_default()
}
