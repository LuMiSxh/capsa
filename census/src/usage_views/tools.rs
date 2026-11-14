// SPDX-License-Identifier: MPL-2.0
//! The `census usage --tools` view: tool categories and the commands that bypass acus.

use super::{k, table};
use capsa_core::render::{Doc, Render};
use census::usage::{CategoryStat, Report, group};
use serde::Serialize;

/// `census usage --tools`: one row per tool category, with its share of the group, as its JSON.
#[derive(Serialize)]
struct ToolRow {
    group: String,
    category: String,
    claude: u64,
    codex: u64,
    total: u64,
    share: f64,
    result_chars: u64,
    result_share: f64,
    result_p50: u64,
    result_p90: u64,
    result_max: u64,
}

/// One command of a category that bypasses acus, as the `--misses` table's JSON.
#[derive(Serialize)]
struct MissRow {
    category: String,
    command: String,
    /// Calls that ran unguarded.
    calls: u64,
    /// Calls of the same shape that the guard denied or rewrote.
    intercepted: u64,
    result_chars: u64,
}

#[derive(Serialize)]
pub(crate) struct ToolsView {
    #[serde(skip)]
    sessions: usize,
    tools: Vec<ToolRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    misses: Option<Vec<MissRow>>,
}

impl ToolsView {
    pub(crate) fn new(r: &Report, misses: bool, top: usize) -> Self {
        ToolsView {
            sessions: r.sessions.len(),
            tools: tool_rows(r),
            misses: misses.then(|| miss_rows(r, top)),
        }
    }
}

impl Render for ToolsView {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --tools");
        doc.line(format!(
            "tool usage: {} sessions; a call counts once per category it hits",
            self.sessions
        ));
        let rows = self
            .tools
            .iter()
            .map(|t| {
                vec![
                    format!("{}/{}", t.group, t.category),
                    t.claude.to_string(),
                    t.codex.to_string(),
                    t.total.to_string(),
                    format!("{:.0}%", 100.0 * t.share),
                    k(t.result_chars as f64),
                    format!("{:.0}%", 100.0 * t.result_share),
                    k(t.result_p50 as f64),
                    k(t.result_p90 as f64),
                    k(t.result_max as f64),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "category",
                "claude",
                "codex",
                "total",
                "share-of-group",
                "result-chars",
                "result-share-of-group",
                "result-p50",
                "result-p90",
                "result-max",
            ],
            "lrrrrrrrrr",
            rows,
        ));
        if let Some(misses) = &self.misses {
            doc.section("commands that bypass acus");
            let rows = misses
                .iter()
                .map(|m| {
                    vec![
                        m.category.clone(),
                        m.command.clone(),
                        m.calls.to_string(),
                        m.intercepted.to_string(),
                        k(m.result_chars as f64),
                    ]
                })
                .collect();
            doc.table(table(
                &[
                    "category",
                    "command",
                    "calls",
                    "intercepted",
                    "result-chars",
                ],
                "llrrr",
                rows,
            ));
        }
    }
}

/// The tool categories, sorted by group and then by calls, with the shares of their group.
fn tool_rows(r: &Report) -> Vec<ToolRow> {
    let mut rows: Vec<_> = r
        .categories
        .iter()
        .map(|(c, s)| (group(c), *c, s))
        .collect();
    rows.sort_by_key(|&(g, c, s)| (g, std::cmp::Reverse(total_calls(s)), c));
    let group_sum = |g: &str, f: fn(&CategoryStat) -> u64| {
        rows.iter()
            .filter(|row| row.0 == g)
            .map(|row| f(row.2))
            .sum::<u64>()
            .max(1)
    };
    rows.iter()
        .map(|&(g, c, s)| {
            let q = s.result_quantiles();
            ToolRow {
                group: g.to_owned(),
                category: c.to_owned(),
                claude: s.calls[0],
                codex: s.calls[1],
                total: total_calls(s),
                share: total_calls(s) as f64 / group_sum(g, total_calls) as f64,
                result_chars: total_chars(s),
                result_share: total_chars(s) as f64 / group_sum(g, total_chars) as f64,
                result_p50: q[0],
                result_p90: q[1],
                result_max: q[2],
            }
        })
        .collect()
}

/// The most frequent bypassing commands, at most `top` of them.
fn miss_rows(r: &Report, top: usize) -> Vec<MissRow> {
    r.misses
        .iter()
        .take(top)
        .map(|m| MissRow {
            category: m.category.to_owned(),
            command: m.command.clone(),
            calls: m.calls,
            intercepted: m.intercepted,
            result_chars: m.result_chars,
        })
        .collect()
}

fn total_calls(s: &CategoryStat) -> u64 {
    s.calls.iter().sum()
}

fn total_chars(s: &CategoryStat) -> u64 {
    s.result_chars.iter().sum()
}
