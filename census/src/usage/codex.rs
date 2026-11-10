// SPDX-License-Identifier: MPL-2.0
//! Parses Codex rollout JSONL transcripts into per-file token and tool-call stats.

use super::time::parse_ts;
use super::{Call, Compaction, FileStats, Request, Tokens, is_explore_cmd, text_chars};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::HashMap;

#[derive(Deserialize)]
struct Rec<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    timestamp: Option<&'a str>,
    #[serde(borrow)]
    payload: Option<Payload<'a>>,
}

#[derive(Deserialize)]
struct Payload<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    cwd: Option<String>,
    model: Option<String>,
    name: Option<String>,
    call_id: Option<&'a str>,
    #[serde(borrow)]
    arguments: Option<&'a RawValue>,
    #[serde(borrow)]
    input: Option<&'a RawValue>,
    #[serde(borrow)]
    output: Option<&'a RawValue>,
    info: Option<Info>,
}

#[derive(Deserialize)]
struct Info {
    total_token_usage: Option<Usage>,
    last_token_usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

pub(crate) fn parse(text: &str) -> FileStats {
    let mut st = FileStats::default();
    let mut model = String::new();
    let mut last_total = None;
    let mut tool_of: HashMap<String, usize> = HashMap::new();
    let mut tasks = 0;
    for line in text.lines() {
        let Ok(r) = serde_json::from_str::<Rec>(line) else {
            continue;
        };
        let ts = r.timestamp.map_or(0, parse_ts);
        if r.kind == "compacted" {
            let pre_tokens = st.requests.last().map_or(0, |q| q.tokens.context());
            st.compactions.push(Compaction { ts, pre_tokens });
        }
        let Some(p) = r.payload else { continue };
        match (r.kind, p.kind) {
            ("session_meta", _) => {
                if st.cwd.is_empty() {
                    st.cwd = p.cwd.unwrap_or_default();
                }
            }
            ("turn_context", _) => {
                if let Some(m) = p.model {
                    model = m;
                }
            }
            ("event_msg", Some("user_message")) => st.prompts += 1,
            ("event_msg", Some("task_started")) => tasks += 1,
            ("event_msg", Some("token_count")) => {
                let Some(info) = p.info else { continue };
                // The same usage is often reported twice; count it when the running total moves.
                let total = info.total_token_usage.map(|t| t.total_tokens);
                if total.is_some() && total == last_total {
                    continue;
                }
                last_total = total;
                let Some(u) = info.last_token_usage else {
                    continue;
                };
                let t = Tokens {
                    input: u
                        .input_tokens
                        .saturating_sub(u.cached_input_tokens + u.cache_write_input_tokens),
                    cache_read: u.cached_input_tokens,
                    cache_write_5m: u.cache_write_input_tokens,
                    cache_write_1h: 0,
                    output: u.output_tokens,
                    requests: 1,
                };
                st.requests.push(Request {
                    id: None,
                    model: model.clone(),
                    tokens: t,
                    ts,
                    after_compact: false,
                });
            }
            ("response_item", Some("function_call" | "custom_tool_call" | "local_shell_call")) => {
                let name = p.name.unwrap_or_else(|| "shell".into());
                let input = p.arguments.or(p.input).map(RawValue::get).unwrap_or("");
                if let Some(id) = p.call_id {
                    tool_of.insert(id.to_owned(), st.calls.len());
                }
                let hits = super::tools::codex_tool(&name, input);
                st.calls.push(Call {
                    categories: hits.categories,
                    commands: hits.commands,
                    input_chars: input.len() as u64,
                    explore: shell_explores(input),
                    name,
                    intercepted: false,
                    result_chars: 0,
                });
            }
            ("response_item", Some("function_call_output" | "custom_tool_call_output")) => {
                let chars = p.output.map_or(0, |o| text_chars(o.get()));
                match p.call_id.and_then(|id| tool_of.get(id)) {
                    Some(&i) => st.calls[i].result_chars += chars,
                    None => st.orphan_result_chars += chars,
                }
            }
            _ => {}
        }
    }
    // Newer Codex versions log no `user_message`; every turn starts a task instead.
    if st.prompts == 0 {
        st.prompts = tasks;
    }
    st
}

/// Finds the command after the first `cmd`/`command` key in a tool input and classifies it.
// ponytail: text heuristic over JSON/JS tool inputs; parse per tool schema if accuracy matters.
fn shell_explores(input: &str) -> bool {
    let Some(i) = input.find("cmd").or_else(|| input.find("command")) else {
        return false;
    };
    let rest = input[i..].trim_start_matches(|c: char| c.is_ascii_alphabetic());
    let rest = rest.trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/');
    is_explore_cmd(rest)
}
