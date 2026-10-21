// SPDX-License-Identifier: MPL-2.0

use super::*;

mod agent_exit;
mod bench;
mod cancel;
mod cancel_hold;
mod capability;
mod cards;
mod entry_order;
mod failsafe;
mod finish;
mod interrupt;
mod notification;
mod permission;
mod replay;
mod resume;
mod roster;
mod steer;
mod steer_hold;
mod transcript;
mod watchdog;

fn test_config() -> Config {
    Config::default()
}

fn engine() -> Engine {
    Engine::new(test_config())
}

fn line(v: Value) -> Vec<u8> {
    serde_json::to_vec(&v).unwrap()
}

fn update(update: Value) -> Vec<u8> {
    line(
        json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s", "update": update}}),
    )
}

fn prompt(id: i64) -> Vec<u8> {
    line(
        json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt", "params": {"sessionId": "s", "prompt": []}}),
    )
}

fn cancel() -> Vec<u8> {
    line(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "s"}}))
}

fn task_call() -> Vec<u8> {
    update(
        json!({"sessionUpdate": "tool_call", "toolCallId": "P", "title": "Task",
            "_meta": {"claudeCode": {"toolName": "Task"}}}),
    )
}

fn child(extra: Value) -> Vec<u8> {
    let mut u = json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Read src/a.rs",
            "_meta": {"claudeCode": {"toolName": "Read", "parentToolUseId": "P"}}});
    u.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    update(u)
}

fn response(id: i64) -> Vec<u8> {
    line(json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "end_turn"}}))
}

/// Builds an engine with prompt 1 outstanding and one running subagent at t0.
fn busy(t0: Instant) -> Engine {
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&task_call(), t0);
    e.on_agent(&child(json!({})), t0);
    e
}

fn client_lines(actions: &[Action]) -> Vec<Value> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::ToClient { line, .. } => serde_json::from_slice(line).ok(),
            _ => None,
        })
        .collect()
}

fn real_parent(id: &str) -> Vec<u8> {
    update(
        json!({"_meta": {"claudeCode": {"toolName": "Agent"}}, "toolCallId": id,
        "sessionUpdate": "tool_call", "name": "Agent", "rawInput": {}, "status": "pending",
        "title": "Task", "kind": "think", "content": []}),
    )
}

fn real_launch_result(id: &str) -> Vec<u8> {
    let text = "Async agent launched successfully. (This tool result is internal metadata)\nagentId: a1 (internal ID)\nThe agent is working in the background.";
    update(
        json!({"_meta": {"claudeCode": {"toolName": "Agent"}}, "toolCallId": id,
        "sessionUpdate": "tool_call_update", "status": "completed",
        "rawOutput": [{"type": "text", "text": text}],
        "content": [{"type": "content", "content": {"type": "text", "text": text}}]}),
    )
}

fn kid(parent: &str, id: &str, title: &str) -> Vec<u8> {
    update(
        json!({"_meta": {"claudeCode": {"toolName": "Read", "parentToolUseId": parent}},
        "toolCallId": id, "sessionUpdate": "tool_call", "status": "pending",
        "title": title, "kind": "read"}),
    )
}

/// Builds a child tool call that already ended (opened and completed): the
/// subagent is not working on anything right now.
fn kid_ended(parent: &str, id: &str, title: &str) -> Vec<u8> {
    update(
        json!({"_meta": {"claudeCode": {"toolName": "Read", "parentToolUseId": parent}},
        "toolCallId": id, "sessionUpdate": "tool_call", "status": "completed",
        "title": title, "kind": "read"}),
    )
}

/// Builds the completion update of a child tool call opened with [`kid`].
fn kid_end(parent: &str, id: &str) -> Vec<u8> {
    update(
        json!({"_meta": {"claudeCode": {"toolName": "Read", "parentToolUseId": parent}},
        "toolCallId": id, "sessionUpdate": "tool_call_update", "status": "completed"}),
    )
}

fn task_notification() -> Vec<u8> {
    update(
        json!({"sessionUpdate": "usage_update", "used": 32489, "size": 1000000,
        "cost": {"amount": 0.97, "currency": "USD"},
        "_meta": {"_claude/origin": {"kind": "task-notification", "producer": "session-task"},
            "_claude/model": "claude-sonnet-5-5"}}),
    )
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// Builds prompt 1 with background subagents `A` and `B` launched at t0 + 1s.
fn two_background(t0: Instant) -> Engine {
    two_background_with(t0, test_config())
}

fn two_background_with(t0: Instant, cfg: Config) -> Engine {
    let mut e = Engine::new(cfg);
    e.on_client(&prompt(1), t0);
    for id in ["A", "B"] {
        e.on_agent(&real_parent(id), t0);
        e.on_agent(&real_launch_result(id), t0 + secs(1));
    }
    e
}

fn updates_for(actions: &[Action], id: &str) -> Vec<Value> {
    client_lines(actions)
        .into_iter()
        .filter(|m| m["params"]["update"]["toolCallId"] == id)
        .collect()
}

/// The "finished" entries among the client lines, in order.
fn entries(actions: &[Action]) -> Vec<Value> {
    client_lines(actions)
        .into_iter()
        .filter(|m| {
            m["params"]["update"]["toolCallId"]
                .as_str()
                .is_some_and(|id| id.starts_with("pons-finished-"))
        })
        .collect()
}

fn card_text(m: &Value) -> String {
    let c = m["params"]["update"]["content"].as_array().unwrap();
    c[c.len() - 1]["content"]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn is_finish_note(a: &Action) -> bool {
    matches!(a, Action::Note(n) if n.contains("finished ("))
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn chunk(text: &str) -> Vec<u8> {
    update(json!({"sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": text}}))
}

fn main_tool_call() -> Vec<u8> {
    update(json!({"sessionUpdate": "tool_call", "toolCallId": "m1", "title": "Bash"}))
}

fn initialize(notices: bool) -> Vec<u8> {
    let session = if notices {
        json!({"notices": {}})
    } else {
        json!({"configOptions": {}})
    };
    line(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": 1, "clientCapabilities": {"session": session}}}))
}

fn of_kind(actions: &[Action], kind: &str) -> Vec<Value> {
    client_lines(actions)
        .into_iter()
        .filter(|m| m["params"]["update"]["sessionUpdate"] == kind)
        .collect()
}

/// Builds prompt 1 outstanding, background subagent `A` launched at t0 and last
/// active at t0 + 1s, so the idle heuristic ends it at t0 + 21s.
fn idle_subagent(t0: Instant) -> Engine {
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(1));
    e.on_agent(&child_text("A", "All clear."), t0 + secs(1));
    e
}

fn is_card_done(m: &Value) -> bool {
    m["params"]["update"]["toolCallId"] == "A" && m["params"]["update"]["status"] == "completed"
}

const NOTE: &str = "<task-notification><task-id>t1</task-id><tool-use-id>toolu_1</tool-use-id>\
    <output-file>/tmp/t1.output</output-file><status>completed</status>\
    <summary>Agent done</summary><note>n</note><result>The result</result>\
    <usage>1</usage></task-notification>";

fn user_chunk(text: &str) -> Vec<u8> {
    update(json!({"sessionUpdate": "user_message_chunk",
        "content": {"type": "text", "text": text}}))
}

fn texts(actions: &[Action]) -> Vec<String> {
    client_lines(actions)
        .iter()
        .filter_map(|m| m["params"]["update"]["content"]["text"].as_str())
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Copy)]
enum Ev {
    Prompt,
    Launch(&'static str),
    Child(&'static str),
    ChildEnd(&'static str),
    Chunk,
    Marker,
    Response,
    /// The follow-up prompt that takes over before the first is answered.
    Prompt2,
}

fn notices_in(actions: &[Action]) -> Vec<Value> {
    of_kind(actions, "notice")
}

/// Sends the notification that ends A at t0 + 10s, with prompt 1 outstanding with A and B.
fn notify_a(e: &mut Engine, t0: Instant) -> Vec<Action> {
    e.on_client(&prompt(1), t0);
    for id in ["A", "B"] {
        e.on_agent(&real_parent(id), t0);
        e.on_agent(&real_launch_result(id), t0);
    }
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(2));
    e.on_agent(&kid_ended("B", "b1", "Read b"), t0 + secs(5));
    e.on_agent(&task_notification(), t0 + secs(10))
}

fn initialize_with(capabilities: Value) -> Vec<u8> {
    line(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": 1, "clientCapabilities": capabilities}}))
}

fn sent_to_client(actions: &[Action]) -> Vec<(Vec<u8>, bool)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::ToClient { line, synthesized } => Some((line.clone(), *synthesized)),
            _ => None,
        })
        .collect()
}

/// Asserts that the lines went to the client untouched, in order.
fn assert_untouched(actions: &[Action], raw: &[u8]) {
    assert_eq!(sent_to_client(actions), vec![(raw.to_vec(), false)]);
}

fn notes(actions: &[Action]) -> Vec<&str> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Note(n) => Some(n.as_str()),
            _ => None,
        })
        .collect()
}

fn load_request(method: &str) -> Vec<u8> {
    line(json!({"jsonrpc": "2.0", "id": "L", "method": method,
        "params": {"sessionId": "s", "cwd": "/x", "mcpServers": []}}))
}

/// Builds the logged replay: user text, then four background
/// launches reported completed in the same millisecond.
fn replayed_history() -> Vec<Vec<u8>> {
    let mut lines = vec![chunk_of("user_message_chunk", "please review")];
    for id in ["A", "B", "C", "D"] {
        lines.push(real_parent(id));
        lines.push(real_launch_result(id));
    }
    lines.push(chunk("all four are running"));
    lines
}

fn chunk_of(kind: &str, text: &str) -> Vec<u8> {
    update(json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}))
}

fn failing_config(max: u32) -> Config {
    Config {
        max_failures: max,
        inject_failure: Some("boom".into()),
        ..test_config()
    }
}

fn boom_request() -> Vec<u8> {
    line(json!({"jsonrpc": "2.0", "id": 9, "method": "x/boom", "params": {"sessionId": "s"}}))
}

/// Returns the `initialize` line that reached the agent, parsed.
fn to_agent(actions: &[Action]) -> Value {
    let sent: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            Action::ToAgent(l) => Some(l),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1);
    serde_json::from_slice(sent[0]).unwrap()
}

fn child_text(parent: &str, text: &str) -> Vec<u8> {
    update(json!({"_meta": {"claudeCode": {"parentToolUseId": parent}},
        "sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": text}}))
}

fn card_texts(actions: &[Action], parent: &str) -> Vec<String> {
    updates_for(actions, parent)
        .iter()
        .filter_map(|m| {
            m["params"]["update"]["content"]
                .as_array()
                .and_then(|c| c.last())
                .and_then(|c| c["content"]["text"].as_str())
                .map(str::to_owned)
        })
        .collect()
}

/// Streams text chunks for a background subagent, as the adapter does.
fn text_subagent(e: &mut Engine, t0: Instant, chunks: &[&str]) {
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(1));
    e.on_agent(&kid_ended("A", "a2", "Grep b"), t0 + secs(1));
    for c in chunks {
        let out = e.on_agent(&child_text("A", c), t0 + secs(2));
        // Child text never goes flat into the main chat.
        assert!(sent_to_client(&out).iter().all(|(_, synth)| *synth));
    }
}

const INTERRUPT: &str = "[Request interrupted by user]";

fn replay_with_marker(e: &mut Engine, t0: Instant) -> Vec<Action> {
    e.on_client(&load_request("session/load"), t0);
    let mut out = Vec::new();
    for raw in [
        chunk_of("user_message_chunk", "before"),
        chunk_of("user_message_chunk", INTERRUPT),
        chunk("after"),
    ] {
        out.extend(e.on_agent(&raw, t0));
    }
    out
}

fn text_prompt(id: i64, text: &str) -> Vec<u8> {
    line(
        json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
        "params": {"sessionId": "s", "prompt": [{"type": "text", "text": text}]}}),
    )
}

/// Builds prompt 1 with five background subagents A..E (real shapes), each with
/// one tool call, and the main agent streaming text at `t0 + 9.9s`.
fn five_background(t0: Instant, cfg: Config) -> Engine {
    let mut e = Engine::new(cfg);
    e.on_client(&prompt(1), t0);
    for id in ["A", "B", "C", "D", "E"] {
        e.on_agent(&real_parent(id), t0);
        e.on_agent(&real_launch_result(id), t0 + secs(1));
        e.on_agent(&kid(id, &format!("{id}1"), "Read a"), t0 + secs(2));
    }
    e.on_agent(&chunk("working"), t0 + ms(9900));
    e
}

/// Builds an engine whose agent advertised steering.
fn steering_engine(t0: Instant, cfg: Config) -> Engine {
    let mut e = five_background(t0, cfg);
    e.on_client(&initialize(false), t0);
    let reply = line(json!({"jsonrpc": "2.0", "id": 0,
        "result": {"_meta": {"steering": {"supported": true}}}}));
    assert!(
        notes(&e.on_agent(&reply, t0))
            .iter()
            .any(|n| n.contains("advertised steering"))
    );
    e
}

fn agent_msgs(actions: &[Action]) -> Vec<Value> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::ToAgent(l) => serde_json::from_slice(l).ok(),
            _ => None,
        })
        .collect()
}

fn steer_reply(id: &str, outcome: Value) -> Vec<u8> {
    line(json!({"jsonrpc": "2.0", "id": id, "result": outcome}))
}

fn running(e: &Engine) -> usize {
    e.folds.running_background("s").len()
}

fn has_note(actions: &[Action], what: &str) -> bool {
    notes(actions).iter().any(|n| n.contains(what))
}

fn failed_or_finished(actions: &[Action]) -> bool {
    client_lines(actions).iter().any(|m| {
        let u = &m["params"]["update"];
        u["status"] == "failed"
            || u["status"] == "completed"
            || u["toolCallId"]
                .as_str()
                .is_some_and(|i| i.starts_with("pons-finished"))
    }) || has_note(actions, "finished (")
}

fn usage() -> Vec<u8> {
    update(json!({"sessionUpdate": "usage_update", "used": 1, "size": 2}))
}

fn pending_call(id: &str) -> Vec<u8> {
    update(
        json!({"sessionUpdate": "tool_call", "toolCallId": id, "title": "Edit a.rs",
        "kind": "edit", "status": "pending", "rawInput": {}}),
    )
}

fn tool_status(id: &str, status: &str) -> Vec<u8> {
    update(json!({"sessionUpdate": "tool_call_update", "toolCallId": id, "status": status}))
}

fn cancelled_response(id: i64) -> Vec<u8> {
    line(json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "cancelled"}}))
}

fn responses(actions: &[Action]) -> Vec<Value> {
    client_lines(actions)
        .into_iter()
        .filter(|m| m.get("result").is_some() || m.get("error").is_some())
        .collect()
}

fn failed_ids(actions: &[Action]) -> Vec<String> {
    client_lines(actions)
        .iter()
        .filter(|m| m["params"]["update"]["status"] == "failed")
        .filter_map(|m| {
            m["params"]["update"]["toolCallId"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

/// Builds prompt 1 answered with text and a usage update, then nothing.
fn finished_text(t0: Instant) -> Engine {
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&chunk("All done."), t0 + secs(1));
    e.on_agent(&usage(), t0 + secs(2));
    e
}

/// Builds a steering engine whose main agent is streaming the input of tool `m1`.
fn streaming_tool(t0: Instant, cfg: Config) -> Engine {
    let mut e = steering_engine(t0, cfg);
    e.on_agent(&pending_call("m1"), t0 + secs(9));
    e
}

fn steer_request(actions: &[Action]) -> Option<Value> {
    agent_msgs(actions)
        .into_iter()
        .find(|m| m["method"] == "_session/steering")
}

fn permission_request(title: &str, kind: &str) -> Vec<u8> {
    line(
        json!({"jsonrpc": "2.0", "id": "perm-9", "method": "session/request_permission",
        "params": {"sessionId": "s",
            "toolCall": {"toolCallId": "t1", "title": title, "kind": kind},
            "options": [
                {"optionId": "always", "name": "Always", "kind": "allow_always"},
                {"optionId": "once", "name": "Allow", "kind": "allow_once"},
                {"optionId": "no", "name": "Reject", "kind": "reject_once"}]}}),
    )
}

fn approving(mode: AutoApprove, tools: &str) -> Engine {
    Engine::new(Config {
        auto_approve: mode,
        auto_approve_tools: tools.to_owned(),
        ..test_config()
    })
}

/// Returns the `sessionUpdate` and status of every "finished" entry line, in order.
fn entry_trail(actions: &[Action]) -> Vec<(String, String)> {
    entries(actions)
        .iter()
        .map(|m| {
            (
                m["params"]["update"]["sessionUpdate"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                m["params"]["update"]["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn trail(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}
