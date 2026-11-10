// SPDX-License-Identifier: MPL-2.0
//! The price table census keeps: the chat models of the providers it prices, in USD per million
//! tokens, trimmed from LiteLLM's price file.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The providers whose chat models the table keeps.
pub const PROVIDERS: [&str; 3] = ["anthropic", "deepseek", "openai"];

/// Cache prices as multiples of the input price where the source omits them: the Anthropic list
/// ratios, cache read 0.1x, 5-minute write 1.25x and 1-hour write 2x.
const CACHE_READ_RATIO: f64 = 0.1;
const WRITE_5M_RATIO: f64 = 1.25;
const WRITE_1H_RATIO: f64 = 2.0;

/// USD per million tokens of one price tier.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
}

/// A long-prompt tier: a request whose prompt has more than `over` tokens pays `price`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tier {
    pub over: u64,
    pub price: Price,
}

/// One model of the table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub provider: String,
    /// The largest input the source names, in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    pub price: Price,
    /// In ascending order of `over`; empty for most models.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<Tier>,
}

impl Entry {
    /// Returns the price of a request whose prompt has `prompt` tokens: the highest tier it exceeds,
    /// else the base price.
    pub fn price_at(&self, prompt: u64) -> Price {
        self.tiers
            .iter()
            .rev()
            .find(|t| prompt > t.over)
            .map_or(self.price, |t| t.price)
    }
}

/// The price table. `fetched_at` is the unix time of the download the table came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub fetched_at: u64,
    /// Keyed by the bare model id.
    pub models: BTreeMap<String, Entry>,
}

/// Trims LiteLLM's price file to the chat models of `PROVIDERS` that have an input and an output
/// price. A bare id wins over the same id with a provider prefix, such as `deepseek/deepseek-flash`.
///
/// # Errors
/// Fails when `text` is not a JSON object.
pub fn trim(text: &str, fetched_at: u64) -> Result<Table, serde_json::Error> {
    let source: BTreeMap<String, Value> = serde_json::from_str(text)?;
    let mut models: BTreeMap<String, Entry> = BTreeMap::new();
    for (key, value) in &source {
        let Some(entry) = value.as_object().and_then(entry_of) else {
            continue;
        };
        match key.split_once('/') {
            Some((_, bare)) => {
                models.entry(bare.to_owned()).or_insert(entry);
            }
            None => {
                models.insert(key.clone(), entry);
            }
        }
    }
    Ok(Table { fetched_at, models })
}

/// Returns the entry of a LiteLLM object, or `None` when it is not a priced chat model of a kept
/// provider.
fn entry_of(obj: &Map<String, Value>) -> Option<Entry> {
    let provider = obj.get("litellm_provider")?.as_str()?;
    if !PROVIDERS.contains(&provider) {
        return None;
    }
    if obj.get("mode").is_some_and(|m| m.as_str() != Some("chat")) {
        return None;
    }
    let price = price_with_suffix(obj, "")?;
    let tiers = obj
        .keys()
        .map(String::as_str)
        .filter_map(tier_over)
        .collect::<BTreeSet<u64>>()
        .into_iter()
        .filter_map(|over| {
            let price = price_with_suffix(obj, &format!("_above_{}k_tokens", over / 1000))?;
            Some(Tier { over, price })
        })
        .collect();
    Some(Entry {
        provider: provider.to_owned(),
        context_window: obj.get("max_input_tokens").and_then(Value::as_u64),
        price,
        tiers,
    })
}

/// Returns the price of the tier whose field names end in `suffix`, in USD per million tokens. A
/// cache price the source omits takes its ratio of the input price.
fn price_with_suffix(obj: &Map<String, Value>, suffix: &str) -> Option<Price> {
    let field = |name: &str| {
        obj.get(&format!("{name}{suffix}"))
            .and_then(Value::as_f64)
            .map(per_million)
    };
    let input = field("input_cost_per_token")?;
    Some(Price {
        input,
        output: field("output_cost_per_token")?,
        cache_read: field("cache_read_input_token_cost")
            .unwrap_or_else(|| round(input * CACHE_READ_RATIO)),
        cache_write_5m: field("cache_creation_input_token_cost")
            .unwrap_or_else(|| round(input * WRITE_5M_RATIO)),
        cache_write_1h: field("cache_creation_input_token_cost_above_1hr")
            .unwrap_or_else(|| round(input * WRITE_1H_RATIO)),
    })
}

/// Returns the prompt size in tokens of a long-tier key, such as
/// `input_cost_per_token_above_100k_tokens`. Batch keys do not end in `_tokens` and are skipped.
fn tier_over(key: &str) -> Option<u64> {
    let (_, size) = key.strip_suffix("_tokens")?.rsplit_once("_above_")?;
    Some(size.strip_suffix('k')?.parse::<u64>().ok()? * 1000)
}

/// Converts a price per token to USD per million tokens.
fn per_million(per_token: f64) -> f64 {
    round(per_token * 1e6)
}

/// Rounds to six decimals, which removes the float noise of the per-token source values.
fn round(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}
