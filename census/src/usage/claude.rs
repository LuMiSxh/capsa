// SPDX-License-Identifier: MPL-2.0
//! Parses Claude Code JSONL transcripts into per-file token and tool-call stats.

use super::time::parse_ts;
use super::{Call, Compaction, FileStats, Request, SpawnCall, Tokens, is_explore_cmd, text_chars};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::HashMap;

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
}

#[derive(Deserialize)]
struct CompactMeta {
    #[serde(rename = "preTokens")]
    pre_tokens: Option<u64>,
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
                    st.calls.push(Call {
                        categories: super::tools::claude_tool(&name, input),
                        input_chars: input.len() as u64,
                        explore,
                        name,
                        result_chars: 0,
                    });
                }
            }
            Some("user") if r.is_meta != Some(true) => {
                user_record(&mut st, &tool_of, content, ts);
            }
            _ => {}
        }
    }
    st
}

fn blocks(content: &str) -> Vec<Block<'_>> {
    if content.starts_with('[') {
        serde_json::from_str(content).unwrap_or_default()
    } else {
        vec![]
    }
}

/// A user record: a prompt, or the results of the tool calls it answers.
fn user_record(st: &mut FileStats, tool_of: &HashMap<String, usize>, content: &str, ts: u64) {
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
