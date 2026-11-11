// SPDX-License-Identifier: MPL-2.0
//! Parses Claude Code JSONL transcripts into per-file token and tool-call stats.

use super::time::parse_ts;
use super::{Call, Compaction, FileStats, Request, SpawnCall, is_explore_cmd, text_chars};
use crate::transcript::{Attachment, Record, records};
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::collections::{HashMap, HashSet};

/// The guard's note on a call it ran rewritten; Claude Code records it as an attachment tied to
/// the call.
const REWRITE_MARKER: &str = "The command was rewritten, not refused";

/// Every acus guard refusal names the acus command to use instead. Another PreToolUse hook's
/// refusal reaches the transcript with the same `hook error:` envelope, so this is what tells
/// them apart.
const GUARD_MARKER: &str = "acus ";

impl Attachment {
    /// Whether this is the guard's note that it ran the call in its rewritten form.
    fn rewrote(&self) -> bool {
        self.kind.as_deref() == Some("hook_additional_context")
            && self
                .content
                .as_ref()
                .and_then(Value::as_array)
                .is_some_and(|lines| {
                    lines
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|c| c.contains(REWRITE_MARKER))
                })
    }
}

#[derive(Deserialize)]
struct Block<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    id: Option<&'a str>,
    name: Option<&'a str>,
    tool_use_id: Option<&'a str>,
    #[serde(borrow)]
    input: Option<&'a RawValue>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

impl Block<'_> {
    /// Whether the result is the guard's refusal, which Claude Code records as a hook error.
    fn guard_denial(&self) -> bool {
        let content = self.content.map(RawValue::get).unwrap_or("");
        let Ok(v) = serde_json::from_str::<Value>(content) else {
            return false;
        };
        v.as_str().is_some_and(|s| {
            s.starts_with("PreToolUse:") && s.contains("hook error:") && s.contains(GUARD_MARKER)
        })
    }
}

#[derive(Deserialize)]
struct BashInput<'a> {
    command: Option<&'a str>,
}

#[derive(Deserialize)]
struct AgentInput<'a> {
    subagent_type: Option<&'a str>,
}

/// The state of one transcript while its records are read in file order.
#[derive(Default)]
struct Parser {
    st: FileStats,
    /// The index in `st.calls` of each call by its tool-use id.
    tool_of: HashMap<String, usize>,
    /// The ids of the calls that the guard denied or rewrote.
    intercepted: HashSet<String>,
    /// Set by a compaction; the next request is the first one after it.
    after_compact: bool,
}

pub(crate) fn parse(text: &str) -> FileStats {
    let mut parser = Parser::default();
    for record in records(text) {
        parser.record(&record);
    }
    parser.finish()
}

impl Parser {
    fn record(&mut self, r: &Record<'_>) {
        if self.st.cwd.is_empty()
            && let Some(cwd) = &r.cwd
        {
            self.st.cwd.clone_from(cwd);
        }
        let ts = r.timestamp.and_then(parse_ts).unwrap_or(0);
        if r.kind == Some("system") && r.subtype == Some("compact_boundary") {
            self.compaction(r, ts);
        }
        if let Some(id) = rewritten_call(&r.attachment) {
            self.intercepted.insert(id.to_owned());
        }
        let Some(message) = &r.message else {
            return;
        };
        let content = message.content.map(RawValue::get).unwrap_or("");
        match r.kind {
            Some("assistant") => self.assistant(r, content, ts),
            Some("user") if r.is_meta != Some(true) => self.user(content, ts),
            _ => {}
        }
    }

    fn compaction(&mut self, r: &Record<'_>, ts: u64) {
        // Transcripts without `compactMetadata` fall back to the last request's context.
        let pre_tokens = r
            .compact
            .as_ref()
            .and_then(|c| c.pre_tokens)
            .unwrap_or_else(|| self.st.requests.last().map_or(0, |q| q.tokens.context()));
        self.st.compactions.push(Compaction { ts, pre_tokens });
        self.after_compact = true;
    }

    fn assistant(&mut self, r: &Record<'_>, content: &str, ts: u64) {
        if let Some(req) = r.request() {
            self.st.requests.push(Request {
                id: req.id,
                model: req.model,
                tokens: req.tokens,
                ts,
                after_compact: std::mem::take(&mut self.after_compact),
            });
        }
        for b in blocks(content).iter().filter(|b| b.kind == "tool_use") {
            self.tool_call(b, ts);
        }
    }

    fn tool_call(&mut self, b: &Block<'_>, ts: u64) {
        let name = b.name.unwrap_or("?").to_owned();
        let input = b.input.map(RawValue::get).unwrap_or("");
        let explore = is_explore(&name, input);
        if let Some(id) = b.id {
            if let Some(role) = spawned_role(&name, input) {
                self.st.spawns.push(SpawnCall {
                    id: id.to_owned(),
                    role,
                    start_ms: ts,
                    end_ms: None,
                });
            }
            self.tool_of.insert(id.to_owned(), self.st.calls.len());
        }
        let hits = super::tools::claude_tool(&name, input);
        self.st.calls.push(Call {
            categories: hits.categories,
            commands: hits.commands,
            input_chars: input.len() as u64,
            explore,
            name,
            intercepted: false,
            result_chars: 0,
        });
    }

    /// A user record: a prompt, or the results of the tool calls it answers.
    fn user(&mut self, content: &str, ts: u64) {
        if content.starts_with('"') {
            self.st.prompts += 1;
            return;
        }
        let bs = blocks(content);
        if bs.iter().any(|b| b.kind == "text") && !bs.iter().any(|b| b.kind == "tool_result") {
            self.st.prompts += 1;
        }
        for b in bs.iter().filter(|b| b.kind == "tool_result") {
            self.tool_result(b, ts);
        }
    }

    fn tool_result(&mut self, b: &Block<'_>, ts: u64) {
        let chars = b.content.map_or(0, |c| text_chars(c.get()));
        if b.guard_denial()
            && let Some(id) = b.tool_use_id
        {
            self.intercepted.insert(id.to_owned());
        }
        match b.tool_use_id.and_then(|id| self.tool_of.get(id).copied()) {
            Some(i) => self.st.calls[i].result_chars += chars,
            None => self.st.orphan_result_chars += chars,
        }
        if let Some(s) = self
            .st
            .spawns
            .iter_mut()
            .rev()
            .find(|s| Some(s.id.as_str()) == b.tool_use_id)
        {
            s.end_ms = Some(ts);
        }
    }

    /// Marks the calls that the guard's denial or rewrite notes name as intercepted.
    fn finish(mut self) -> FileStats {
        for id in &self.intercepted {
            if let Some(&i) = self.tool_of.get(id) {
                self.st.calls[i].intercepted = true;
            }
        }
        self.st
    }
}

/// Whether a tool call only reads: a file read or search, or a shell command that does the same.
fn is_explore(name: &str, input: &str) -> bool {
    match name {
        "Read" | "Grep" | "Glob" | "LS" => true,
        "Bash" => serde_json::from_str::<BashInput>(input)
            .ok()
            .and_then(|i| i.command)
            .is_some_and(is_explore_cmd),
        _ => false,
    }
}

/// The role of an `Agent` or `Task` call that spawns a subagent.
fn spawned_role(name: &str, input: &str) -> Option<String> {
    if !matches!(name, "Agent" | "Task") {
        return None;
    }
    let role = serde_json::from_str::<AgentInput>(input)
        .ok()?
        .subagent_type?;
    Some(role.to_owned())
}

/// The call whose rewrite an attachment records, when it is the guard's rewrite note.
fn rewritten_call(attachment: &Option<Attachment>) -> Option<&str> {
    let a = attachment.as_ref()?;
    a.rewrote().then_some(a.tool_use_id.as_deref()).flatten()
}

fn blocks(content: &str) -> Vec<Block<'_>> {
    if content.starts_with('[') {
        serde_json::from_str(content).unwrap_or_default()
    } else {
        vec![]
    }
}
