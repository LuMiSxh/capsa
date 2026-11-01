// SPDX-License-Identifier: MPL-2.0
//! Holds the value types of the ledger: requests, hourly aggregates, token sums and filters.

use serde::{Deserialize, Serialize};

/// Records one API request of one agent thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// Message id; one request can span several transcript records.
    pub id: String,
    /// Unix seconds.
    pub t: u64,
    pub session: String,
    /// Subagent id; `None` is the main agent.
    pub agent: Option<String>,
    /// Subagent type as Claude Code attributes it, such as `Explore`.
    pub role: Option<String>,
    pub model: String,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    /// Seconds since the previous request of the same thread; `None` for the first.
    pub gap_s: Option<u64>,
}

/// Sums token classes over requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    pub requests: u64,
}

impl Tokens {
    pub(crate) fn add_row(&mut self, r: &Row) {
        self.input += r.input;
        self.cache_read += r.cache_read;
        self.cache_write_5m += r.cache_write_5m;
        self.cache_write_1h += r.cache_write_1h;
        self.output += r.output;
        self.requests += 1;
    }

    pub(crate) fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.cache_read += o.cache_read;
        self.cache_write_5m += o.cache_write_5m;
        self.cache_write_1h += o.cache_write_1h;
        self.output += o.output;
        self.requests += o.requests;
    }
}

/// Aggregates the requests of one hour, model family and main/sub split.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hourly {
    /// Unix seconds of the hour start.
    pub hour: u64,
    pub family: String,
    pub sub: bool,
    #[serde(flatten)]
    pub tokens: Tokens,
}

/// Selects requests by time, thread kind and model family.
#[derive(Clone, Debug)]
pub struct Filter {
    /// First second included.
    pub from: u64,
    /// First second excluded.
    pub to: u64,
    /// `Some(true)` keeps subagents, `Some(false)` the main agent.
    pub sub: Option<bool>,
    /// Model family as [`family_of`] names it.
    pub family: Option<String>,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            from: 0,
            to: u64::MAX,
            sub: None,
            family: None,
        }
    }
}

impl Filter {
    pub(crate) fn keeps(&self, t: u64, sub: bool, family: &str) -> bool {
        (self.from..self.to).contains(&t)
            && self.sub.is_none_or(|s| s == sub)
            && self.family.as_deref().is_none_or(|f| f == family)
    }
}

/// Names the model family used to group aggregates: `opus`, `sonnet`, `haiku` or `other`.
pub fn family_of(model: &str) -> &'static str {
    ["opus", "sonnet", "haiku"]
        .into_iter()
        .find(|f| model.contains(f))
        .unwrap_or("other")
}

/// Rounds a time down to the start of its hour.
pub(crate) fn floor_hour(t: u64) -> u64 {
    t / 3600 * 3600
}
