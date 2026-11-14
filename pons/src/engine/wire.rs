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

/// Tells whether `update` is the usage update the adapter sends after a turn it started
/// on its own for a finished background task. It does not say which task ended.
pub(super) fn is_task_notification(update: UpdateView<'_>) -> bool {
    update.session_update == Some("usage_update")
        && update
            .meta
            .and_then(|m| m.get("_claude/origin"))
            .and_then(|o| o.get("kind"))
            .and_then(Value::as_str)
            == Some("task-notification")
}

/// Tells whether `update` is the adapter's result of an `Agent` call that went to the
/// background.
pub(super) fn is_async_launch(update: UpdateView<'_>) -> bool {
    const MARKER: &str = "Async agent launched";
    let starts = |v: &Value| {
        v.get("text")
            .and_then(Value::as_str)
            .is_some_and(|t| t.starts_with(MARKER))
    };
    let raw = update.raw().get("rawOutput");
    raw.and_then(Value::as_str)
        .is_some_and(|t| t.starts_with(MARKER))
        || raw
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(starts))
        || update
            .content_items()
            .is_some_and(|a| a.iter().any(|c| c.get("content").is_some_and(starts)))
}

/// Guesses from the name whether a tool call spawns a subagent. The real adapter sends
/// `_meta.claudeCode.toolName` and a top-level `name` of `Agent` with title
/// `Task` and kind `think`. A tool call these miss is still treated as a
/// parent once a child names it in `parentToolUseId`.
pub(super) fn is_subagent_tool(update: UpdateView<'_>) -> bool {
    let named = |v: Option<&str>| matches!(v, Some("Task" | "Agent"));
    named(update.tool_name)
        || named(update.name)
        || (named(update.title) && update.kind == Some("think"))
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

pub(super) fn track_tool(
    session: &mut Session,
    id: &str,
    kind: Option<&str>,
    update: UpdateView<'_>,
) {
    let status = update.status;
    if update.is_terminal() {
        session.open_tools.remove(id);
        return;
    }
    let text = |v: Option<&str>| v.map(str::to_owned);
    match kind {
        Some("tool_call") => {
            let has_input = update.has_input();
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
                    title: text(update.title).unwrap_or_default(),
                    kind: text(update.kind).unwrap_or_default(),
                    name: text(update.name.or(update.tool_name)).unwrap_or_default(),
                },
            );
        }
        Some("tool_call_update") => {
            if let Some(tool) = session.open_tools.get_mut(id) {
                // A status change, or the refinement that carries the content
                // built from the complete input, ends the streaming phase.
                if status.is_some_and(|s| s != "pending") || update.content.is_some() {
                    tool.streaming = false;
                }
                if let Some(title) = text(update.title) {
                    tool.title = title;
                }
                if let Some(kind) = text(update.kind) {
                    tool.kind = kind;
                }
            }
        }
        _ => {}
    }
}

/// Describes a child tool call as it starts, so a stalled subagent can be found
/// in the wire log.
pub(super) fn child_start_note(card: &str, update: UpdateView<'_>) -> String {
    let title = clip_head(&one_line(update.title.unwrap_or_default()), 80);
    let kind = match update.kind.unwrap_or_default() {
        "" => String::new(),
        kind => format!(" (kind {kind})"),
    };
    format!(
        "subagent card {card}: child tool call {} started: {title}{kind}",
        update.tool_call_id.unwrap_or_default()
    )
}
