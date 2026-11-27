// SPDX-License-Identifier: MPL-2.0
//! Owns the output anatomy, the cache-gap buckets and the context-size buckets of a report.

use super::Cost;
use serde::Serialize;

/// The bucket boundaries of the gap between two requests of a thread, in milliseconds.
pub(super) const GAP_BUCKETS: [(u64, &str); 4] = [
    (5 * 60 * 1000, "<5m"),
    (60 * 60 * 1000, "5-60m"),
    (4 * 60 * 60 * 1000, "1-4h"),
    (u64::MAX, ">4h"),
];

/// A cache write after a pause counts as a rewrite when the pause is over five minutes and the
/// write is over 5k tokens, as the content had to be written again.
pub(super) const REWRITE_GAP_MS: u64 = 5 * 60 * 1000;
pub(super) const REWRITE_TOKENS: u64 = 5_000;

/// The bucket boundaries of a request's context size, in tokens.
pub(super) const CONTEXT_BUCKETS: [(u64, &str); 6] = [
    (50_000, "<50k"),
    (100_000, "50-100k"),
    (150_000, "100-150k"),
    (200_000, "150-200k"),
    (300_000, "200-300k"),
    (u64::MAX, ">300k"),
];

/// Output tokens split into thinking, visible text and tool-call JSON. `output` is the provider's
/// count; text and tool characters are estimates at four characters per token, so `residual` holds
/// the tokenizer overhead and the reasoning a provider does not report apart.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct OutputStat {
    pub requests: u64,
    pub output: u64,
    pub thinking: u64,
    pub text_chars: u64,
    pub tool_chars: u64,
}

impl OutputStat {
    /// Returns the visible text in tokens, at four characters per token.
    pub fn text_tokens(&self) -> u64 {
        self.text_chars / 4
    }

    /// Returns the tool-call input in tokens, at four characters per token.
    pub fn tool_tokens(&self) -> u64 {
        self.tool_chars / 4
    }

    /// Returns what the three buckets leave of `output`, which is negative when the estimates
    /// exceed the provider's count.
    pub fn residual(&self) -> i64 {
        self.output as i64
            - self.thinking as i64
            - self.text_tokens() as i64
            - self.tool_tokens() as i64
    }
}

/// The requests of one gap bucket: how much cache they re-wrote after the pause, how often a write
/// was a rewrite, and what those rewrites cost.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct GapBucket {
    pub label: &'static str,
    pub requests: u64,
    pub cache_read: u64,
    pub cache_writes: u64,
    pub output: u64,
    pub rewrites: u64,
    pub rewrite_usd: f64,
}

/// The requests of one role at one context size: their cost and the cache they read and wrote.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct ContextBucket {
    pub role: &'static str,
    pub label: &'static str,
    pub requests: u64,
    pub cost: Cost,
    pub cache_read: u64,
    pub cache_writes: u64,
    pub output: u64,
}

/// Returns the index of the first boundary at or above `n`.
pub(super) fn bucket_of(bounds: &[(u64, &'static str)], n: u64) -> usize {
    bounds
        .iter()
        .position(|(max, _)| n <= *max)
        .unwrap_or(bounds.len() - 1)
}
