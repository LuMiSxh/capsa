// SPDX-License-Identifier: MPL-2.0
//! Owns what one request costs at list prices. The table comes from the price cache, which is
//! fetched from LiteLLM, or from the snapshot built into the binary.

mod source;
mod table;
#[cfg(test)]
mod tests;

use super::Tokens;
use crate::ledger::family_of;
use capsa_core::paths::state_dir;
use serde::Serialize;
use std::sync::OnceLock;
pub use table::PROVIDERS;
use table::{Price, Table};

/// Refreshes the cached price list when it is older than `days`. The usage, limit and session
/// commands call it before they price anything. A failed fetch keeps the cache, logs a warning and
/// is not retried for an hour. The hook and the status line never fetch, and
/// `CENSUS_NO_PRICE_REFRESH` turns the refresh off.
pub fn refresh(now: u64, days: u64) {
    if std::env::var_os("CENSUS_NO_PRICE_REFRESH").is_some() {
        return;
    }
    source::refresh(&state_dir("census"), now, days);
}

/// Returns the table in use: the cache when it reads, else the snapshot. It is loaded on first use.
/// Unit tests use the snapshot, so they do not depend on the machine's cache.
fn current() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        if cfg!(test) {
            source::snapshot()
        } else {
            source::load(&state_dir("census"))
        }
    })
}

/// Returns the fetch time of the table in use. The ledger digest records it, so a new price list
/// prices the digest again.
pub(crate) fn fetched_at() -> u64 {
    current().fetched_at
}

/// Returns the provider of a priced model, such as `anthropic`, or `None` for an unknown model.
pub(crate) fn provider(model: &str) -> Option<&'static str> {
    current().models.get(model).map(|e| e.provider.as_str())
}

/// Returns the provider that bills a model. An unpriced model is `anthropic` when its id starts
/// with `claude` (a Claude model the price list does not know yet), else `unknown`.
pub(crate) fn billed_by(model: &str) -> &'static str {
    provider(model).unwrap_or(if model.starts_with("claude") {
        "anthropic"
    } else {
        crate::UNKNOWN
    })
}

/// Returns whether a request of `model` counts toward the Claude rate-limit windows and the budget
/// month. Only `anthropic` does, whatever `billing` in the config says: the windows track Claude's
/// own limits, so a provider set to subscription does not move them.
pub(crate) fn counts_toward_limits(model: &str) -> bool {
    billed_by(model) == "anthropic"
}

/// Returns whether the price table lists a model of `provider`.
pub fn knows_provider(provider: &str) -> bool {
    current().models.values().any(|e| e.provider == provider)
}

/// The list-price cost of one request in USD, or `None` for a model without a price.
pub fn request_cost(model: &str, t: &Tokens) -> Option<f64> {
    Some(at_price(price(model, t.context())?, t))
}

/// The base-tier list price of a model family, for aggregated tokens whose model is gone. The
/// long-context tier cannot apply to them. `None` for a family without a price.
pub(crate) fn family_cost(family: &str, t: &Tokens) -> Option<f64> {
    Some(at_price(family_price(family)?, t))
}

/// The USD of the cache writes of one request, or `None` for a model without a price.
pub fn write_cost(model: &str, t: &Tokens) -> Option<f64> {
    price(model, t.context()).map(|p| writes(p, t))
}

/// The price of a model for a prompt of `prompt` tokens, or `None` for a model without one.
fn price(model: &str, prompt: u64) -> Option<Price> {
    Some(current().models.get(model)?.price_at(prompt))
}

/// The base price of the newest Anthropic model of a family: the last one in name order, so
/// `claude-opus-5-5` over `claude-opus-5`. The family `other` has no price.
fn family_price(family: &str) -> Option<Price> {
    if family == "other" {
        return None;
    }
    current()
        .models
        .iter()
        .rev()
        .find(|(m, e)| e.provider == "anthropic" && family_of(m) == family)
        .map(|(_, e)| e.price)
}

/// USD of the tokens of one request at `p`.
fn at_price(p: Price, t: &Tokens) -> f64 {
    let per_token =
        t.input as f64 * p.input + t.cache_read as f64 * p.cache_read + t.output as f64 * p.output;
    per_token / 1e6 + writes(p, t)
}

/// USD of the cache writes of one request at `p`.
fn writes(p: Price, t: &Tokens) -> f64 {
    let per_million =
        t.cache_write_5m as f64 * p.cache_write_5m + t.cache_write_1h as f64 * p.cache_write_1h;
    per_million / 1e6
}

/// List-price dollars of some requests, and how many of them had no price.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct Cost {
    pub usd: f64,
    pub unpriced: u64,
}

impl Cost {
    /// Adds one request. `<synthetic>` entries are not model calls and are not counted.
    pub fn add_request(&mut self, model: &str, t: &Tokens) {
        if model == "<synthetic>" {
            return;
        }
        match request_cost(model, t) {
            Some(usd) => self.usd += usd,
            None => self.unpriced += 1,
        }
    }

    pub fn merge(&mut self, o: &Cost) {
        self.usd += o.usd;
        self.unpriced += o.unpriced;
    }
}
