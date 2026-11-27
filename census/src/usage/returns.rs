// SPDX-License-Identifier: MPL-2.0
//! Owns the subagent-return statistics: launch results, completion notifications and final words.

use super::{AgentStat, FileStats, SpawnCall, percentile};
use serde::Serialize;
use std::collections::HashMap;

/// One role's subagent returns. The launch result is what the `Agent` call printed when it started;
/// the notification is the final report that Claude Code injected when the subagent stopped.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ReturnStat {
    pub role: String,
    pub calls: u64,
    /// Spawn calls with a subagent transcript to read.
    pub transcripts: u64,
    pub launch_p50: u64,
    pub launch_p90: u64,
    pub notifications: u64,
    pub notification_p50: u64,
    pub notification_p90: u64,
    /// Words of the last assistant text of each subagent transcript.
    pub final_words_p50: u64,
    pub final_words_p90: u64,
    pub final_words_total: u64,
    pub output: u64,
    pub thinking: u64,
    pub thinking_share: f64,
}

/// The raw values of one role, kept until the report is complete.
#[derive(Default)]
pub(super) struct ReturnAcc {
    calls: u64,
    transcripts: u64,
    launch: Vec<u64>,
    notifications: Vec<u64>,
    final_words: Vec<u64>,
}

pub(super) fn add_spawn(acc: &mut HashMap<String, ReturnAcc>, c: &SpawnCall) {
    let a = acc.entry(c.role.clone()).or_default();
    a.calls += 1;
    if c.result_chars > 0 {
        a.launch.push(c.result_chars);
    }
    if let Some(chars) = c.notification_chars.filter(|&n| n > 0) {
        a.notifications.push(chars);
    }
}

pub(super) fn add_file(acc: &mut HashMap<String, ReturnAcc>, f: &FileStats) {
    if !f.is_subagent {
        return;
    }
    let a = acc.entry(f.role.clone()).or_default();
    a.transcripts += 1;
    if f.final_words > 0 {
        a.final_words.push(f.final_words);
    }
}

/// Returns one row per role, with the output and thinking that the role's agent row already counted.
pub(super) fn finish(acc: HashMap<String, ReturnAcc>, agents: &[AgentStat]) -> Vec<ReturnStat> {
    let mut rows: Vec<ReturnStat> = acc
        .into_iter()
        .map(|(role, mut a)| {
            let agent = agents.iter().find(|x| x.role == role);
            let output = agent.map_or(0, |x| x.tokens.output);
            let thinking = agent.map_or(0, |x| x.thinking);
            let (launch_p50, launch_p90) = pair(&mut a.launch);
            let (notification_p50, notification_p90) = pair(&mut a.notifications);
            let (final_words_p50, final_words_p90) = pair(&mut a.final_words);
            ReturnStat {
                role,
                calls: a.calls,
                transcripts: a.transcripts,
                launch_p50,
                launch_p90,
                notifications: a.notifications.len() as u64,
                notification_p50,
                notification_p90,
                final_words_p50,
                final_words_p90,
                final_words_total: a.final_words.iter().sum(),
                output,
                thinking,
                thinking_share: if output > 0 {
                    thinking as f64 / output as f64
                } else {
                    0.0
                },
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.output
            .cmp(&a.output)
            .then(b.calls.cmp(&a.calls))
            .then(a.role.cmp(&b.role))
    });
    rows
}

/// Sorts `values` and returns the p50 and p90, or 0 each when it is empty.
fn pair(values: &mut [u64]) -> (u64, u64) {
    values.sort_unstable();
    (percentile(values, 0.5), percentile(values, 0.9))
}
