// SPDX-License-Identifier: MPL-2.0
//! Tiny scripted ACP agent for the integration tests. Reads and writes
//! newline-delimited JSON on stdio.
//!
//! - any request `echo/*`: answers `{"echoed": params}`
//! - request `ask`: sends the client a `session/request_permission` request
//!   (id "perm-1"), answers `ask` with the client's reply
//! - request `initialize`: answers `{"echoed": params}`, like `echo/*`
//! - request `session/load`: replays a subagent text chunk (as the adapter does
//!   for a client with the transcript capability), a user message, an
//!   interrupt marker as a user message, and a main agent chunk, then answers
//!   `{}`
//! - `session/prompt` with text `subagent`: emits a Task tool call and three
//!   subagent updates, then keeps the turn open
//! - `session/prompt` with text `subagent-result`: like `subagent`, but the
//!   subagent's text arrives in two chunks after a second tool call
//! - `session/prompt` with text `background-result`: like `background-notify`,
//!   with the subagent's text in two chunks before the notification
//! - `session/prompt` with text `background` or `background-notify`: launches a
//!   background Agent (reported completed at once, as the real adapter does),
//!   runs one subagent tool call, and for `background-notify` then sends the
//!   `task-notification` usage update; the turn stays open
//! - `session/prompt` with text `background-text`: like `background`, then the
//!   subagent's final text (after its tool call, no notification); the turn
//!   stays open
//! - `session/prompt` with text `spawn-grandchild`: spawns `mock_agent --sleep`
//!   and reports `mock/pids` (`agent`, `grandchild`)
//! - argument `--sleep`: sleeps forever (the grandchild)
//! - argument `--ignore-eof`: ignores SIGTERM (unix) and keeps running after
//!   stdin closes, so only SIGKILL stops it
//! - `session/prompt` with text `stream`: like `background`, then the main agent
//!   streams eight text chunks 100 ms apart; the turn stays open
//! - `session/prompt` with text `finish`: answers every outstanding prompt
//! - `session/prompt` with text `final-text`: a text chunk and a usage update,
//!   then the turn stays open (an adapter turn that never ends)
//! - `session/prompt` with text `tool-open`: a pending `Edit` tool call (`m1`,
//!   empty input); the turn stays open
//! - `session/prompt` with text `background-tool`: like `background`, then a
//!   pending main-agent tool call `m1` whose input "completes" 500 ms later
//!   (an `in_progress` update from a thread)
//! - `session/prompt` with text `die`: the agent exits with code 3, unanswered
//! - `session/prompt` with text `permission-read` or `permission-bash`: asks
//!   the client for permission (id "perm-2", a `Read` or `Bash` tool call
//!   with allow_once, allow_always and reject_once options) and reports the
//!   answer as `mock/permission_seen` (its `result`)
//! - `session/prompt` with text `flood:<path>`: writes 3 MiB of text chunks
//!   without pause, creates the file `<path>`, then answers the prompt (a
//!   client that does not read must not stall the agent)
//! - argument `--cancel-ends`: `session/cancel` also answers every outstanding
//!   prompt with `{stopReason: cancelled}`
//! - other prompts: stay open silently
//! - `session/cancel`: reports `mock/cancel_seen` to the client
//! - argument `--steer MODE` (`injected`, `started`, `prompt-required` or
//!   `error`): `initialize` also advertises `_meta.steering.supported`, and
//!   the request `_session/steering` reports `mock/steer_seen` (its params)
//!   to the client, then answers `{outcome: injected}`, `{outcome:
//!   startedNewTurn}`, `{outcome: promptRequired}` or a JSON-RPC error. With
//!   `injected` and the text `finish` the held turn then ends: every
//!   outstanding prompt is answered with `{stopReason: end_turn, steered:
//!   true}`. Without the argument the method is unknown (an error).

#![expect(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    reason = "test fixture: a failed setup should panic, and update() takes its payload by value"
)]

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// The rate-limit event as claude-agent-acp forwards it; the census test expects this line.
const RATE_LIMIT_LINE: &str = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"usage_update","used":1,"size":2,"_meta":{"_claude/rateLimit":{"status":"allowed","resetsAt":1793530800,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.45,"resetsAt":1793530800},"seven_day":{"utilization":0.05,"resetsAt":1794106800}}}}}}}"#;

fn send(v: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn update(sid: &str, update: Value) {
    send(&json!({"jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": sid, "update": update}}));
}

fn child_meta(tool: &str) -> Value {
    json!({"claudeCode": {"toolName": tool, "parentToolUseId": "toolu_parent"}})
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--sleep") {
        sleep_forever();
    }
    let ignore_eof = args.iter().any(|a| a == "--ignore-eof");
    let cancel_ends = args.iter().any(|a| a == "--cancel-ends");
    let steer = args
        .iter()
        .position(|a| a == "--steer")
        .and_then(|i| args.get(i + 1))
        .cloned();
    #[cfg(unix)]
    if ignore_eof {
        ignore_sigterm();
    }
    let mut prompts: Vec<Value> = Vec::new();
    let mut ask_id: Option<Value> = None;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let sid = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("s1")
            .to_owned();
        match (method, id) {
            (Some("ask"), Some(id)) => {
                ask_id = Some(id);
                send(&json!({"jsonrpc": "2.0", "id": "perm-1",
                    "method": "session/request_permission", "params": {"sessionId": sid}}));
            }
            (Some(m), Some(id)) if m.starts_with("echo/") => {
                send(&json!({"jsonrpc": "2.0", "id": id, "result": {"echoed": params}}));
            }
            (Some("initialize"), Some(id)) => initialize(&id, &params, steer.is_some()),
            (Some("_session/steering"), Some(id)) => {
                steering(&id, &params, steer.as_deref(), &mut prompts);
            }
            (Some("session/load"), Some(id)) => replay(&sid, &id),
            (Some("session/prompt"), Some(id)) => {
                let text = params
                    .pointer("/prompt/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                prompts.push(id);
                prompt_turn(&sid, text, &mut prompts);
            }
            (Some("session/cancel"), None) => {
                send(&json!({"jsonrpc": "2.0", "method": "mock/cancel_seen",
                    "params": {"sessionId": sid}}));
                if cancel_ends {
                    answer_all(&mut prompts, "cancelled");
                }
            }
            (None, Some(id)) if id == "perm-2" => {
                send(&json!({"jsonrpc": "2.0", "method": "mock/permission_seen",
                    "params": {"result": msg.get("result").cloned().unwrap_or(Value::Null)}}));
            }
            (None, Some(id)) if id == "perm-1" => {
                if let Some(ask) = ask_id.take() {
                    let result = msg.get("result").cloned().unwrap_or(Value::Null);
                    send(&json!({"jsonrpc": "2.0", "id": ask, "result": {"permission": result}}));
                }
            }
            _ => {}
        }
    }
    // stdin closed. A well-behaved agent exits; the stubborn one does not.
    if ignore_eof {
        sleep_forever();
    }
}

fn sleep_forever() -> ! {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

#[cfg(unix)]
fn ignore_sigterm() {
    // SAFETY: installing SIG_IGN for SIGTERM is async-signal-safe and has no
    // preconditions.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
}

fn initialize(id: &Value, params: &Value, steering: bool) {
    let mut result = json!({"echoed": params});
    if steering {
        result["_meta"] = json!({"steering": {"supported": true}});
    }
    send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn steering(id: &Value, params: &Value, mode: Option<&str>, prompts: &mut Vec<Value>) {
    let Some(mode) = mode else {
        send(&json!({"jsonrpc": "2.0", "id": id,
            "error": {"code": -32601, "message": "Method not found"}}));
        return;
    };
    send(&json!({"jsonrpc": "2.0", "method": "mock/steer_seen", "params": params}));
    match mode {
        "injected" => {
            send(&json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": "injected"}}));
            if params.pointer("/prompt/0/text").and_then(Value::as_str) == Some("finish") {
                for prompt_id in prompts.drain(..) {
                    send(&json!({"jsonrpc": "2.0", "id": prompt_id,
                        "result": {"stopReason": "end_turn", "steered": true}}));
                }
            }
        }
        "started" => {
            send(&json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": "startedNewTurn"}}));
        }
        "prompt-required" => send(&json!({"jsonrpc": "2.0", "id": id,
            "result": {"outcome": "promptRequired", "reason": "noRunningTurn"}})),
        _ => send(&json!({"jsonrpc": "2.0", "id": id,
            "error": {"code": -32603, "message": "steering failed"}})),
    }
}

fn replay(sid: &str, id: &Value) {
    update(
        sid,
        json!({"sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "replayed subagent text"},
            "_meta": child_meta("Task")}),
    );
    update(
        sid,
        json!({"sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "replayed user text"}}),
    );
    update(
        sid,
        json!({"sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "[Request interrupted by user]"}}),
    );
    update(
        sid,
        json!({"sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "replayed main text"}}),
    );
    send(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
}

fn prompt_turn(sid: &str, text: &str, prompts: &mut Vec<Value>) {
    match text {
        "subagent" | "subagent-result" => subagent_turn(sid, text),
        "spawn-grandchild" => spawn_grandchild(),
        "final-text" => {
            update(
                sid,
                json!({"sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "All done."}}),
            );
            update(
                sid,
                json!({"sessionUpdate": "usage_update", "used": 1, "size": 2}),
            );
        }
        "tool-open" => update(
            sid,
            json!({"sessionUpdate": "tool_call", "toolCallId": "m1",
                "title": "Edit a.rs", "kind": "edit", "status": "pending",
                "rawInput": {}}),
        ),
        "rate-limit" => println!("{RATE_LIMIT_LINE}"),
        "die" => std::process::exit(3),
        t if t.starts_with("flood:") => flood(sid, &t["flood:".len()..], prompts),
        "permission-read" | "permission-bash" => permission_turn(sid, text),
        "background" | "background-notify" | "background-result" | "background-text"
        | "background-tool" | "stream" => background_turn(sid, text),
        "finish" => answer_all(prompts, "end_turn"),
        _ => {}
    }
}

fn answer_all(prompts: &mut Vec<Value>, reason: &str) {
    for id in prompts.drain(..) {
        send(&json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": reason}}));
    }
}

fn subagent_turn(sid: &str, text: &str) {
    update(
        sid,
        json!({"sessionUpdate": "tool_call", "toolCallId": "toolu_parent",
            "title": "Task", "status": "pending",
            "_meta": {"claudeCode": {"toolName": "Task"}}}),
    );
    update(
        sid,
        json!({"sessionUpdate": "tool_call", "toolCallId": "toolu_read",
            "title": "Read src/a.rs", "status": "pending",
            "_meta": child_meta("Read")}),
    );
    update(
        sid,
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "toolu_read",
            "status": "completed", "_meta": child_meta("Read")}),
    );
    update(
        sid,
        json!({"sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "reviewing the file"},
            "_meta": child_meta("Task")}),
    );
    if text == "subagent-result" {
        update(
            sid,
            json!({"sessionUpdate": "tool_call", "toolCallId": "toolu_grep",
                "title": "Grep x", "status": "pending",
                "_meta": child_meta("Grep")}),
        );
        for part in ["Result: ", "all clear."] {
            update(
                sid,
                json!({"sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": part},
                    "_meta": child_meta("Task")}),
            );
        }
        update(
            sid,
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "toolu_parent",
                "status": "completed"}),
        );
    }
}

#[expect(
    clippy::zombie_processes,
    reason = "the test wants the grandchild to outlive pons"
)]
fn spawn_grandchild() {
    let grandchild = std::process::Command::new(std::env::current_exe().expect("current exe"))
        .arg("--sleep")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn grandchild");
    send(&json!({"jsonrpc": "2.0", "method": "mock/pids",
        "params": {"agent": std::process::id(), "grandchild": grandchild.id()}}));
}

fn flood(sid: &str, path: &str, prompts: &mut Vec<Value>) {
    let filler = "x".repeat(16 * 1024);
    for _ in 0..192 {
        update(
            sid,
            json!({"sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": filler}}),
        );
    }
    let _ = std::fs::write(path, "done");
    answer_all(prompts, "end_turn");
}

fn permission_turn(sid: &str, text: &str) {
    let (title, kind) = if text == "permission-read" {
        ("Read a.rs", "read")
    } else {
        ("Bash ls", "execute")
    };
    send(&json!({"jsonrpc": "2.0", "id": "perm-2",
        "method": "session/request_permission",
        "params": {"sessionId": sid,
            "toolCall": {"toolCallId": "t-perm", "title": title, "kind": kind},
            "options": [
                {"optionId": "allow-always", "name": "Always", "kind": "allow_always"},
                {"optionId": "allow-once", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}}));
}

fn background_turn(sid: &str, text: &str) {
    let bg_child = json!({"claudeCode": {"toolName": "Read", "parentToolUseId": "toolu_bg"}});
    update(
        sid,
        json!({"sessionUpdate": "tool_call", "toolCallId": "toolu_bg",
            "name": "Agent", "title": "Task", "kind": "think",
            "status": "pending", "rawInput": {}, "content": [],
            "_meta": {"claudeCode": {"toolName": "Agent"}}}),
    );
    let launch = "Async agent launched successfully. (This tool result is internal metadata)";
    update(
        sid,
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "toolu_bg",
            "status": "completed",
            "rawOutput": [{"type": "text", "text": launch}],
            "content": [{"type": "content", "content": {"type": "text", "text": launch}}],
            "_meta": {"claudeCode": {"toolName": "Agent"}}}),
    );
    update(
        sid,
        json!({"sessionUpdate": "tool_call", "toolCallId": "toolu_bg_read",
            "title": "Read src/b.rs", "status": "pending", "_meta": bg_child}),
    );
    update(
        sid,
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "toolu_bg_read",
            "status": "completed", "_meta": bg_child}),
    );
    if text == "background-tool" {
        update(
            sid,
            json!({"sessionUpdate": "tool_call", "toolCallId": "m1",
                "title": "Edit a.rs", "kind": "edit", "status": "pending",
                "rawInput": {}}),
        );
        let thread_sid = sid.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            update(
                &thread_sid,
                json!({"sessionUpdate": "tool_call_update",
                    "toolCallId": "m1", "status": "in_progress"}),
            );
        });
    }
    if text == "background-text" || text == "stream" {
        update(
            sid,
            json!({"sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "All clear."},
                "_meta": bg_child}),
        );
    }
    if text == "stream" {
        for n in 1..=8 {
            update(
                sid,
                json!({"sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": format!("chunk {n} ")}}),
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    if text == "background-result" {
        for part in ["All ", "clear."] {
            update(
                sid,
                json!({"sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": part},
                    "_meta": bg_child}),
            );
        }
    }
    if text == "background-notify" || text == "background-result" {
        update(
            sid,
            json!({"sessionUpdate": "usage_update", "used": 1, "size": 2,
                "_meta": {"_claude/origin": {"kind": "task-notification",
                    "producer": "session-task"}}}),
        );
    }
}
