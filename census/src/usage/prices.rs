// SPDX-License-Identifier: MPL-2.0
//! Owns the list prices census knows, and what one request costs at them.

use super::Tokens;
use serde::Serialize;

/// USD per million tokens of one price tier.
#[derive(Clone, Copy)]
struct Price {
    input: f64,
    output: f64,
    cache_read: f64,
}

/// Models with a list price. Cache writes cost 1.25x the input price for 5 minutes and 2x for 1 hour.
const PRICES: [(&str, Price); 7] = [
    (
        "claude-opus-5-5",
        Price {
            input: 4.0,
            output: 20.0,
            cache_read: 0.20,
        },
    ),
    (
        "claude-sonnet-5-5",
        Price {
            input: 2.0,
            output: 10.0,
            cache_read: 0.20,
        },
    ),
    (
        "claude-haiku-5-5",
        Price {
            input: 0.10,
            output: 0.50,
            cache_read: 0.01,
        },
    ),
    (
        "claude-fable-5-1",
        Price {
            input: 10.0,
            output: 50.0,
            cache_read: 0.25,
        },
    ),
    (
        "claude-fable-5",
        Price {
            input: 10.0,
            output: 50.0,
            cache_read: 0.25,
        },
    ),
    (
        "claude-opus-5",
        Price {
            input: 5.0,
            output: 25.0,
            cache_read: 0.50,
        },
    ),
    (
        "claude-sonnet-5",
        Price {
            input: 2.0,
            output: 10.0,
            cache_read: 0.20,
        },
    ),
];

/// Haiku 5.5 requests with more than this many context tokens (input, cache read and cache
/// write together) pay the long-prompt tier.
const HAIKU: &str = "claude-haiku-5-5";
const HAIKU_LONG_CONTEXT: u64 = 100_000;
const HAIKU_LONG: Price = Price {
    input: 0.50,
    output: 2.50,
    cache_read: 0.05,
};

fn price(model: &str, context: u64) -> Option<Price> {
    if model == HAIKU && context > HAIKU_LONG_CONTEXT {
        return Some(HAIKU_LONG);
    }
    PRICES.iter().find(|(m, _)| *m == model).map(|(_, p)| *p)
}

/// USD of the cache writes of one request at `p`.
fn writes(p: Price, t: &Tokens) -> f64 {
    (t.cache_write_5m as f64 * 1.25 + t.cache_write_1h as f64 * 2.0) * p.input / 1e6
}

/// The list price of one request in USD, or `None` for a model without a price.
pub fn request_cost(model: &str, t: &Tokens) -> Option<f64> {
    Some(at_price(price(model, t.context())?, t))
}

/// The base-tier list price of a model family, for aggregated tokens whose model is gone. The
/// long-context tier cannot apply to them. `None` for a family without a price.
pub(crate) fn family_cost(family: &str, t: &Tokens) -> Option<f64> {
    if family == "other" {
        return None;
    }
    let (_, p) = PRICES
        .iter()
        .find(|(m, _)| crate::ledger::family_of(m) == family)?;
    Some(at_price(*p, t))
}

/// USD of the tokens of one request at `p`.
fn at_price(p: Price, t: &Tokens) -> f64 {
    let per_token =
        t.input as f64 * p.input + t.cache_read as f64 * p.cache_read + t.output as f64 * p.output;
    per_token / 1e6 + writes(p, t)
}

/// The USD of the cache writes of one request, or `None` for a model without a price.
pub fn write_cost(model: &str, t: &Tokens) -> Option<f64> {
    price(model, t.context()).map(|p| writes(p, t))
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
