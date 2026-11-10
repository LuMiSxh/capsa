// SPDX-License-Identifier: MPL-2.0
//! Parses Claude Code JSONL transcripts into per-file token and tool-call stats.

use super::time::parse_ts;
use super::{Call, Compaction, FileStats, Request, SpawnCall, Tokens, is_explore_cmd, text_chars};
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

#[derive(Deserialize)]
struct Rec<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    subtype: Option<&'a str>,
    cwd: Option<String>,
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
    #[serde(borrow)]
    message: Option<Msg<'a>>,
    timestamp: Option<&'a str>,
    #[serde(rename = "compactMetadata")]
    compact: Option<CompactMeta>,
    attachment: Option<Attachment>,
}

#[derive(Deserialize)]
struct CompactMeta {
    #[serde(rename = "preTokens")]
    pre_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Attachment {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "toolUseID")]
    tool_use_id: Option<String>,
    content: Option<Vec<String>>,
}

impl Attachment {
    /// Whether this is the guard's note that it ran the call in its rewritten form.
    fn rewrote(&self) -> bool {
        self.kind.as_deref() == Some("hook_additional_context")
            && self
                .content
                .iter()
                .flatten()
                .any(|c| c.contains(REWRITE_MARKER))
    }
}

#[derive(Deserialize)]
struct Msg<'a> {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
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

pub(crate) fn parse(text: &str) -> FileStats {
    let mut st = FileStats::default();
    let mut tool_of: HashMap<String, usize> = HashMap::new();
    let mut intercepted: HashSet<String> = HashSet::new();
    let mut after_compact = false;
    for line in text.lines() {
        let Ok(r) = serde_json::from_str::<Rec>(line) else {
            continue;
        };
        if st.cwd.is_empty()
            && let Some(c) = r.cwd
        {
            st.cwd = c;
        }
        let ts = r.timestamp.map_or(0, parse_ts);
        if r.kind == Some("system") && r.subtype == Some("compact_boundary") {
            // Transcripts without `compactMetadata` fall back to the last request's context.
            let pre_tokens = r
                .compact
                .and_then(|c| c.pre_tokens)
                .unwrap_or_else(|| st.requests.last().map_or(0, |q| q.tokens.context()));
            st.compactions.push(Compaction { ts, pre_tokens });
            after_compact = true;
        }
        if let Some(id) = rewritten_call(&r.attachment) {
            intercepted.insert(id.to_owned());
        }
        let Some(m) = r.message else { continue };
        let content = m.content.map(RawValue::get).unwrap_or("");
        match r.kind {
            Some("assistant") => {
                // `<synthetic>` messages are aborts and errors, not API requests.
                let synthetic = m.model.as_deref() == Some("<synthetic>");
                if let Some(u) = m.usage.filter(|_| !synthetic) {
                    let (w5, w1h) = match u.cache_creation {
                        Some(c) => (c.ephemeral_5m_input_tokens, c.ephemeral_1h_input_tokens),
                        None => (u.cache_creation_input_tokens, 0),
                    };
                    let t = Tokens {
                        input: u.input_tokens,
                        cache_read: u.cache_read_input_tokens,
                        cache_write_5m: w5,
                        cache_write_1h: w1h,
                        output: u.output_tokens,
                        requests: 1,
                    };
                    st.requests.push(Request {
                        id: m.id,
                        model: m.model.unwrap_or_default(),
                        tokens: t,
                        ts,
                        after_compact: std::mem::take(&mut after_compact),
                    });
                }
                for b in blocks(content).into_iter().filter(|b| b.kind == "tool_use") {
                    let name = b.name.unwrap_or("?").to_owned();
                    let input = b.input.map(RawValue::get).unwrap_or("");
                    let explore = match name.as_str() {
                        "Read" | "Grep" | "Glob" | "LS" => true,
                        "Bash" => serde_json::from_str::<BashInput>(input)
                            .ok()
                            .and_then(|i| i.command)
                            .is_some_and(is_explore_cmd),
                        _ => false,
                    };
                    if let Some(id) = b.id {
                        if matches!(name.as_str(), "Agent" | "Task")
                            && let Some(t) = serde_json::from_str::<AgentInput>(input)
                                .ok()
                                .and_then(|i| i.subagent_type)
                        {
                            st.spawns.push(SpawnCall {
                                id: id.to_owned(),
                                role: t.to_owned(),
                                start_ms: ts,
                                end_ms: None,
                            });
                        }
                        tool_of.insert(id.to_owned(), st.calls.len());
                    }
                    let hits = super::tools::claude_tool(&name, input);
                    st.calls.push(Call {
                        categories: hits.categories,
                        commands: hits.commands,
                        input_chars: input.len() as u64,
                        explore,
                        name,
                        intercepted: false,
                        result_chars: 0,
                    });
                }
            }
            Some("user") if r.is_meta != Some(true) => {
                user_record(&mut st, &tool_of, &mut intercepted, content, ts);
            }
            _ => {}
        }
    }
    mark_intercepted(&mut st, &tool_of, &intercepted);
    st
}

/// The call whose rewrite an attachment records, when it is the guard's rewrite note.
fn rewritten_call(attachment: &Option<Attachment>) -> Option<&str> {
    let a = attachment.as_ref()?;
    a.rewrote().then_some(a.tool_use_id.as_deref()).flatten()
}

/// Marks the calls that the guard's denial or rewrite notes name as intercepted.
fn mark_intercepted(st: &mut FileStats, tool_of: &HashMap<String, usize>, ids: &HashSet<String>) {
    for id in ids {
        if let Some(&i) = tool_of.get(id) {
            st.calls[i].intercepted = true;
        }
    }
}

fn blocks(content: &str) -> Vec<Block<'_>> {
    if content.starts_with('[') {
        serde_json::from_str(content).unwrap_or_default()
    } else {
        vec![]
    }
}

/// A user record: a prompt, or the results of the tool calls it answers.
fn user_record(
    st: &mut FileStats,
    tool_of: &HashMap<String, usize>,
    intercepted: &mut HashSet<String>,
    content: &str,
    ts: u64,
) {
    if content.starts_with('"') {
        st.prompts += 1;
        return;
    }
    let bs = blocks(content);
    if bs.iter().any(|b| b.kind == "text") && !bs.iter().any(|b| b.kind == "tool_result") {
        st.prompts += 1;
    }
    for b in bs.iter().filter(|b| b.kind == "tool_result") {
        let chars = b.content.map_or(0, |c| text_chars(c.get()));
        if b.guard_denial()
            && let Some(id) = b.tool_use_id
        {
            intercepted.insert(id.to_owned());
        }
        match b.tool_use_id.and_then(|id| tool_of.get(id)) {
            Some(&i) => st.calls[i].result_chars += chars,
            None => st.orphan_result_chars += chars,
        }
        if let Some(s) = st
            .spawns
            .iter_mut()
            .rev()
            .find(|s| Some(s.id.as_str()) == b.tool_use_id)
        {
            s.end_ms = Some(ts);
        }
    }
}
