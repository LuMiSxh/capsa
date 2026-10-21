// SPDX-License-Identifier: MPL-2.0
//! Helpers for the shape of ACP messages.

use super::*;

/// Adds `clientCapabilities._meta["subagent-transcript"] = true` to an
/// `initialize` request, creating the objects on the way. False (nothing
/// changed) when the capability is already set or an existing value is not
/// an object that can hold it.
pub(super) fn inject_transcript(msg: &mut Value) -> bool {
    const KEY: &str = "subagent-transcript";
    let Some(params) = msg.get_mut("params").and_then(Value::as_object_mut) else {
        return false;
    };
    let caps = params
        .entry("clientCapabilities")
        .or_insert_with(|| json!({}));
    if caps.is_null() {
        *caps = json!({});
    }
    let Some(caps) = caps.as_object_mut() else {
        return false;
    };
    let meta = caps.entry("_meta").or_insert_with(|| json!({}));
    if meta.is_null() {
        *meta = json!({});
    }
    let Some(meta) = meta.as_object_mut() else {
        return false;
    };
    if meta.get(KEY).is_some_and(|v| !v.is_null()) {
        return false;
    }
    meta.insert(KEY.to_owned(), Value::Bool(true));
    true
}

pub(super) fn parent_tool_use_id<'a>(msg: &'a Value, update: &'a Value) -> Option<&'a str> {
    update
        .pointer("/_meta/claudeCode/parentToolUseId")
        .or_else(|| msg.pointer("/params/_meta/claudeCode/parentToolUseId"))?
        .as_str()
}

/// Tells whether `update` is the usage update the adapter sends after a turn it started
/// on its own for a finished background task. It does not say which task ended.
pub(super) fn is_task_notification(update: &Value) -> bool {
    update.get("sessionUpdate").and_then(Value::as_str) == Some("usage_update")
        && update
            .pointer("/_meta")
            .and_then(|m| m.get("_claude/origin"))
            .and_then(|o| o.get("kind"))
            .and_then(Value::as_str)
            == Some("task-notification")
}

/// Tells whether `update` is the adapter's result of an `Agent` call that went to the
/// background.
pub(super) fn is_async_launch(update: &Value) -> bool {
    const MARKER: &str = "Async agent launched";
    let starts = |v: &Value| {
        v.get("text")
            .and_then(Value::as_str)
            .is_some_and(|t| t.starts_with(MARKER))
    };
    let raw = update.get("rawOutput");
    raw.and_then(Value::as_str)
        .is_some_and(|t| t.starts_with(MARKER))
        || raw
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(starts))
        || update
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|c| c.get("content").is_some_and(starts)))
}

/// Guesses from the name whether a tool call spawns a subagent. The real adapter sends
/// `_meta.claudeCode.toolName` and a top-level `name` of `Agent` with title
/// `Task` and kind `think`. A tool call these miss is still treated as a
/// parent once a child names it in `parentToolUseId`.
pub(super) fn is_subagent_tool(update: &Value) -> bool {
    let named = |v: Option<&Value>| matches!(v.and_then(Value::as_str), Some("Task" | "Agent"));
    named(update.pointer("/_meta/claudeCode/toolName"))
        || named(update.get("name"))
        || (named(update.get("title"))
            && update.get("kind").and_then(Value::as_str) == Some("think"))
}

pub(super) fn session_id(msg: &Value) -> Option<&str> {
    msg.pointer("/params/sessionId")?.as_str()
}

pub(super) fn method(msg: &Value) -> Option<&str> {
    msg.get("method")?.as_str()
}

/// Returns the session a raw line belongs to, if it parses and names one.
pub(super) fn quick_session(raw: &[u8]) -> Option<String> {
    let msg: Value = serde_json::from_slice(raw).ok()?;
    session_id(&msg).map(str::to_owned)
}

pub(super) fn plain(msg: &Value) -> Action {
    Action::ToClient {
        line: serde_json::to_vec(msg).unwrap_or_default(),
        synthesized: false,
    }
}

/// Tells whether the client set a capability to something other than null/false.
pub(super) fn present(v: Option<&Value>) -> bool {
    v.is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
}

pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

pub(super) fn synthesized(msg: &Value) -> Action {
    Action::ToClient {
        line: serde_json::to_vec(msg).unwrap_or_default(),
        synthesized: true,
    }
}

pub(super) fn track_tool(session: &mut Session, id: &str, kind: Option<&str>, update: &Value) {
    let status = update.get("status").and_then(Value::as_str);
    if matches!(status, Some("completed" | "failed")) {
        session.open_tools.remove(id);
        return;
    }
    let text = |key: &str| update.get(key).and_then(Value::as_str).map(str::to_owned);
    match kind {
        Some("tool_call") => {
            let has_input = match update.get("rawInput") {
                Some(Value::Object(o)) => !o.is_empty(),
                Some(Value::String(s)) => !s.is_empty(),
                Some(Value::Null) | None => false,
                Some(_) => true,
            };
            let prompt = session
                .prompt_id
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_default();
            session.open_tools.insert(
                id.to_owned(),
                OpenTool {
                    prompt,
                    streaming: status.is_none_or(|s| s == "pending") && !has_input,
                    title: text("title").unwrap_or_default(),
                    kind: text("kind").unwrap_or_default(),
                    name: text("name")
                        .or_else(|| {
                            update
                                .pointer("/_meta/claudeCode/toolName")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or_default(),
                },
            );
        }
        Some("tool_call_update") => {
            if let Some(tool) = session.open_tools.get_mut(id) {
                // A status change, or the refinement that carries the content
                // built from the complete input, ends the streaming phase.
                if status.is_some_and(|s| s != "pending") || update.get("content").is_some() {
                    tool.streaming = false;
                }
                if let Some(title) = text("title") {
                    tool.title = title;
                }
                if let Some(kind) = text("kind") {
                    tool.kind = kind;
                }
            }
        }
        _ => {}
    }
}

/// Describes a child tool call as it starts, so a stalled subagent can be found
/// in the wire log.
pub(super) fn child_start_note(card: &str, update: &Value) -> String {
    let text = |key: &str| update.get(key).and_then(Value::as_str).unwrap_or_default();
    let title = clip_head(&one_line(text("title")), 80);
    let kind = match text("kind") {
        "" => String::new(),
        kind => format!(" (kind {kind})"),
    };
    format!(
        "subagent card {card}: child tool call {} started: {title}{kind}",
        text("toolCallId")
    )
}
