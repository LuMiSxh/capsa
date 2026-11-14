// SPDX-License-Identifier: MPL-2.0
//! Borrowed view of a `session/update` payload, read once and used for inspection only.

use std::time::Duration;

use serde_json::Value;

use super::MONITOR_MAX;

/// Holds the fields of an update that pons decides on, borrowed from the parsed message.
/// A field of another type than the one expected reads as absent, as it did before the
/// view existed. Forwarding never goes through it: lines leave as the bytes that came in.
#[derive(Clone, Copy)]
pub struct UpdateView<'a> {
    raw: &'a Value,
    pub session_update: Option<&'a str>,
    pub tool_call_id: Option<&'a str>,
    pub status: Option<&'a str>,
    pub title: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub name: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    pub meta: Option<&'a Value>,
    pub content: Option<&'a Value>,
    pub raw_input: Option<&'a Value>,
    /// `_meta.claudeCode.parentToolUseId` as the update carries it, of any type.
    parent: Option<&'a Value>,
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str()
}

impl<'a> UpdateView<'a> {
    pub fn new(update: &'a Value) -> Self {
        let meta = update.get("_meta");
        let claude = meta.and_then(|m| m.get("claudeCode"));
        Self {
            raw: update,
            session_update: text(update, "sessionUpdate"),
            tool_call_id: text(update, "toolCallId"),
            status: text(update, "status"),
            title: text(update, "title"),
            kind: text(update, "kind"),
            name: text(update, "name"),
            tool_name: claude.and_then(|c| text(c, "toolName")),
            meta,
            content: update.get("content"),
            raw_input: update.get("rawInput"),
            parent: claude.and_then(|c| c.get("parentToolUseId")),
        }
    }

    /// Returns the update of a `session/update` message; a view of nothing when it has none.
    pub fn of_message(msg: &'a Value) -> Self {
        Self::new(msg.pointer("/params/update").unwrap_or(&Value::Null))
    }

    /// Returns the payload the view reads.
    pub fn raw(&self) -> &'a Value {
        self.raw
    }

    /// Returns the subagent this update belongs to: the one named in the update's own
    /// `_meta`, else in the message's `params._meta`. A non-text value in the first is final.
    pub fn parent_tool_use_id(&self, msg: &'a Value) -> Option<&'a str> {
        self.parent
            .or_else(|| msg.pointer("/params/_meta/claudeCode/parentToolUseId"))?
            .as_str()
    }

    /// Tells whether the update ends its tool call.
    pub fn is_terminal(&self) -> bool {
        matches!(self.status, Some("completed" | "failed"))
    }

    /// Returns the text of an `agent_message_chunk`.
    pub fn chunk_text(&self) -> Option<&'a str> {
        self.content?.get("text")?.as_str()
    }

    /// Returns the update's `content` when it is an array.
    pub fn content_items(&self) -> Option<&'a Vec<Value>> {
        self.content?.as_array()
    }

    /// Returns the `rawInput` when it is a non-empty object.
    pub fn input_object(&self) -> Option<&'a Value> {
        self.raw_input
            .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
    }

    /// Tells whether the `rawInput` holds anything: a non-empty object or text, or any
    /// other non-null value.
    pub fn has_input(&self) -> bool {
        match self.raw_input {
            Some(Value::Object(o)) => !o.is_empty(),
            Some(Value::String(s)) => !s.is_empty(),
            Some(Value::Null) | None => false,
            Some(_) => true,
        }
    }

    /// Tells whether the update carries a `title` key, whatever its type.
    pub fn has_title_key(&self) -> bool {
        self.raw.get("title").is_some()
    }

    /// Tells whether a tool call is `Monitor`, by its tool name, else by its name or title.
    pub fn is_monitor(&self) -> bool {
        self.tool_name.or(self.name).or(self.title) == Some("Monitor")
    }

    /// Reads the timeout of a `Monitor` call from its `rawInput`, in milliseconds or seconds.
    // ponytail: the field names are not checked against a captured Monitor call; `timeout_ms`
    // is the one expected, and the others are accepted in case the input names it differently.
    // To confirm, run a Monitor call with `--log` and compare its `rawInput` in the wire log.
    pub fn monitor_timeout(&self) -> Option<Duration> {
        let input = self.raw_input?;
        let ms = |key: &str| input.get(key)?.as_u64().map(Duration::from_millis);
        let secs = |key: &str| input.get(key)?.as_u64().map(Duration::from_secs);
        ms("timeout_ms")
            .or_else(|| ms("timeout"))
            .or_else(|| secs("timeout_seconds"))
            .or_else(|| secs("timeoutSeconds"))
            .map(|t| t.min(MONITOR_MAX))
    }
}

impl<'a> From<&'a Value> for UpdateView<'a> {
    fn from(update: &'a Value) -> Self {
        Self::new(update)
    }
}
