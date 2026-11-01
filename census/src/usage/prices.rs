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
            cache_read: 0.10,
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

/// A price that applies to a request whose prompt is over `over` tokens. The prompt is its input,
/// cache read and cache write tokens together.
#[derive(Clone, Copy)]
struct LongTier {
    over: u64,
    price: Price,
}

/// Models with a long-prompt tier. A request over the threshold pays the tier instead of the base
/// price in `PRICES`.
const LONG_TIERS: [(&str, LongTier); 1] = [(
    "claude-haiku-5-5",
    LongTier {
        over: 100_000,
        price: Price {
            input: 0.50,
            output: 2.50,
            cache_read: 0.05,
        },
    },
)];

/// The price of one request with a prompt of `context` tokens, or `None` for a model without one.
fn price(model: &str, context: u64) -> Option<Price> {
    let long = LONG_TIERS.iter().find(|(m, _)| *m == model).map(|(_, l)| l);
    if let Some(l) = long.filter(|l| context > l.over) {
        return Some(l.price);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A request whose cache writes are all 1-hour ones.
    fn request(input: u64, cache_read: u64, cache_write_1h: u64, output: u64) -> Tokens {
        Tokens {
            input,
            cache_read,
            cache_write_5m: 0,
            cache_write_1h,
            output,
            requests: 1,
        }
    }

    fn usd(model: &str, t: &Tokens) -> f64 {
        request_cost(model, t).expect("a priced model")
    }

    fn assert_usd(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn sonnet_5_5_cache_read_is_a_tenth_of_a_dollar_per_million() {
        assert_usd(usd("claude-sonnet-5-5", &request(0, 1_000_000, 0, 0)), 0.10);
    }

    #[test]
    fn haiku_prompt_of_50k_pays_the_short_tier() {
        // 40k input at $0.10, 10k cache read at $0.01 and 1k output at $0.50, per million.
        let t = request(40_000, 10_000, 0, 1_000);
        assert_usd(usd("claude-haiku-5-5", &t), 0.0046);
    }

    #[test]
    fn haiku_prompt_of_120k_pays_the_long_tier() {
        // 100k input at $0.50, 10k cache read at $0.05, 10k 1h writes at $1.00 and 1k output at
        // $2.50, per million.
        let t = request(100_000, 10_000, 10_000, 1_000);
        assert_usd(usd("claude-haiku-5-5", &t), 0.063);
    }

    #[test]
    fn haiku_prompt_of_exactly_100k_pays_the_short_tier() {
        assert_usd(usd("claude-haiku-5-5", &request(100_000, 0, 0, 0)), 0.01);
    }
}
