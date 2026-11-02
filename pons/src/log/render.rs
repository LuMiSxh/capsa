// SPDX-License-Identifier: MPL-2.0

//! Turns the lines of the wire log into the text that is written.

use super::*;

/// Renders the text to log for one message at the given level.
pub(super) fn render(level: LogLevel, dir: Dir, text: &str) -> String {
    match level {
        LogLevel::All => text.to_owned(),
        LogLevel::Default => {
            if matches!(dir, Dir::AgentToClient | Dir::PonsToClient)
                && let Some(summary) = summarize_update(text)
            {
                return summary;
            }
            text.to_owned()
        }
        LogLevel::Meta => match dir {
            // Pons' own notes are its decisions.
            Dir::Pons => text.to_owned(),
            _ => method_only(text),
        },
    }
}

/// Summarizes a `session/update` notification in one line, `None` for anything
/// else (including unparsable lines).
pub(super) fn summarize_update(text: &str) -> Option<String> {
    if !text.contains("session/update") {
        return None;
    }
    summarize_message(&serde_json::from_str(text).ok()?)
}

/// Summarizes an already parsed message like [`summarize_update`].
pub fn summarize_message(msg: &Value) -> Option<String> {
    if msg.get("method")?.as_str()? != "session/update" {
        return None;
    }
    let update = msg.pointer("/params/update").unwrap_or(&Value::Null);
    let field = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
    let parent = field(
        update
            .pointer("/_meta/claudeCode/parentToolUseId")
            .or_else(|| msg.pointer("/params/_meta/claudeCode/parentToolUseId")),
    );
    let mut out = String::from("session/update");
    for (key, value) in [
        ("session", field(msg.pointer("/params/sessionId"))),
        ("kind", field(update.get("sessionUpdate"))),
        ("toolCallId", field(update.get("toolCallId"))),
        ("status", field(update.get("status"))),
        ("parentToolUseId", parent),
        (
            "origin",
            field(
                update
                    .pointer("/_meta")
                    .and_then(|m| m.get("_claude/origin"))
                    .and_then(|o| o.get("kind")),
            ),
        ),
    ] {
        if let Some(v) = value {
            out.push_str(&format!(" {key}={v}"));
        }
    }
    out.push_str(&format!(" textLen={}", text_len(update)));
    Some(out)
}

/// Counts the characters of text carried by an update: `content.text` for message
/// chunks, `content[*].content.text` for tool call contents.
pub(super) fn text_len(update: &Value) -> usize {
    let chars = |v: Option<&Value>| v.and_then(Value::as_str).map_or(0, |s| s.chars().count());
    let content = update.get("content");
    let direct = chars(content.and_then(|c| c.get("text")));
    let nested: usize = content
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|i| chars(i.pointer("/content/text")))
                .sum()
        })
        .unwrap_or(0);
    direct + nested
}

/// Describes a message as `method=NAME` for requests and notifications, `response id=ID`
/// otherwise.
pub(super) fn method_only(text: &str) -> String {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return "unparsable".to_owned();
    };
    match msg.get("method").and_then(Value::as_str) {
        Some(m) => format!("method={m}"),
        None => format!(
            "response id={}",
            msg.get("id").map_or("none".to_owned(), Value::to_string)
        ),
    }
}
