// SPDX-License-Identifier: MPL-2.0

use super::source::{self, PriceError};
use super::table::{self, Entry, Price, Table};
use super::*;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const DAY_S: u64 = 86_400;
const HOUR_S: u64 = 3_600;

/// The LiteLLM shape for the parser: a prefixed key, a long tier, a missing cache field, a batch
/// key, a non-chat entry, a provider census does not price and a non-model object.
const LITELLM: &str = r#"{
  "sample_spec": {"max_tokens": 1},
  "claude-haiku-5-5": {
    "litellm_provider": "anthropic", "mode": "chat", "max_input_tokens": 1000000,
    "input_cost_per_token": 1e-7, "output_cost_per_token": 5e-7,
    "cache_read_input_token_cost": 1e-8, "cache_creation_input_token_cost": 1.25e-7,
    "cache_creation_input_token_cost_above_1hr": 2e-7,
    "input_cost_per_token_above_100k_tokens": 5e-7,
    "output_cost_per_token_above_100k_tokens": 2.5e-6,
    "cache_read_input_token_cost_above_100k_tokens": 5e-8,
    "cache_creation_input_token_cost_above_100k_tokens": 6.25e-7,
    "cache_creation_input_token_cost_above_1hr_above_100k_tokens": 1e-6,
    "input_cost_per_token_above_100k_tokens_batches": 2.5e-7
  },
  "deepseek/deepseek-flash": {
    "litellm_provider": "deepseek", "mode": "chat",
    "input_cost_per_token": 3e-7, "output_cost_per_token": 1.2e-6,
    "cache_read_input_token_cost": 6e-9
  },
  "text-embedding-3": {
    "litellm_provider": "openai", "mode": "embedding",
    "input_cost_per_token": 2e-8, "output_cost_per_token": 0.0
  },
  "gemini-2.5-pro": {
    "litellm_provider": "vertex_ai-language-models", "mode": "chat",
    "input_cost_per_token": 1.25e-6, "output_cost_per_token": 1e-5
  }
}"#;

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

fn entry(provider: &str, input: f64) -> Entry {
    Entry {
        provider: provider.to_owned(),
        context_window: None,
        price: Price {
            input,
            output: input * 5.0,
            cache_read: 0.0,
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
        },
        tiers: Vec::new(),
    }
}

fn cached(fetched_at: u64, model: &str) -> Table {
    Table {
        fetched_at,
        models: BTreeMap::from([(model.to_owned(), entry("anthropic", 1.0))]),
    }
}

fn write_cache(dir: &Path, t: &Table) {
    let bytes = serde_json::to_vec(t).expect("serialize");
    std::fs::write(dir.join("prices.json"), bytes).expect("write the cache");
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

#[test]
fn opus_5_5_output_is_twenty_dollars_per_million() {
    assert_usd(usd("claude-opus-5-5", &request(0, 0, 0, 1_000_000)), 20.0);
}

#[test]
fn one_hour_cache_writes_cost_twice_the_input_price() {
    assert_usd(usd("claude-sonnet-5-5", &request(0, 0, 1_000_000, 0)), 4.0);
}

#[test]
fn five_minute_cache_writes_cost_one_and_a_quarter_times_the_input_price() {
    let t = Tokens {
        input: 0,
        cache_read: 0,
        cache_write_5m: 1_000_000,
        cache_write_1h: 0,
        output: 0,
        requests: 1,
    };
    assert_usd(usd("claude-sonnet-5-5", &t), 2.5);
}

#[test]
fn a_model_without_a_price_has_no_cost() {
    let t = request(1, 1, 1, 1);
    assert_eq!(request_cost("mystery-model", &t), None);
    assert_eq!(write_cost("mystery-model", &t), None);
    let mut cost = Cost::default();
    cost.add_request("mystery-model", &t);
    assert_usd(cost.usd, 0.0);
    assert_eq!(cost.unpriced, 1);
}

#[test]
fn a_model_without_a_price_is_billed_by_its_id() {
    assert_eq!(billed_by("claude-sonnet-9-0"), "anthropic");
    assert_eq!(billed_by("deepseek-v4-pro"), "deepseek");
    assert_eq!(billed_by("mystery-model"), "unknown");
    assert!(counts_toward_limits("claude-sonnet-9-0"));
    assert!(!counts_toward_limits("deepseek-v4-pro"));
    assert!(!counts_toward_limits("mystery-model"));
}

#[test]
fn provider_names_the_provider_of_a_priced_model() {
    assert_eq!(provider("claude-haiku-5-5"), Some("anthropic"));
    assert_eq!(provider("mystery-model"), None);
}

#[test]
fn a_family_is_priced_at_its_newest_anthropic_model() {
    let haiku = family_cost("haiku", &request(1_000_000, 0, 0, 0));
    assert_usd(haiku.expect("a priced family"), 0.10);
    let sonnet = family_cost("sonnet", &request(0, 0, 0, 1_000_000));
    assert_usd(sonnet.expect("a priced family"), 10.0);
    assert_eq!(family_cost("other", &request(1, 0, 0, 0)), None);
}

#[test]
fn the_snapshot_prices_the_claude_models_census_priced() {
    let snapshot = source::snapshot();
    for model in [
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "claude-fable-5-1",
        "claude-fable-5",
        "claude-opus-5",
        "claude-sonnet-5",
    ] {
        assert!(snapshot.models.contains_key(model), "{model} is missing");
    }
}

#[test]
fn trims_to_the_priced_chat_models() {
    let t = table::trim(LITELLM, 42).expect("parse");
    assert_eq!(t.fetched_at, 42);
    let keys: Vec<&str> = t.models.keys().map(String::as_str).collect();
    assert_eq!(keys, ["claude-haiku-5-5", "deepseek-flash"]);
}

#[test]
fn a_prefixed_key_is_stored_under_its_bare_id() {
    let t = table::trim(LITELLM, 42).expect("parse");
    let e = &t.models["deepseek-flash"];
    assert_eq!(e.provider, "deepseek");
    assert_usd(e.price.input, 0.3);
}

#[test]
fn a_bare_id_wins_over_a_prefixed_one_in_either_order() {
    let text = r#"{
      "a/x": {"litellm_provider": "deepseek", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
      "x": {"litellm_provider": "deepseek", "input_cost_per_token": 3e-6, "output_cost_per_token": 4e-6},
      "z/x": {"litellm_provider": "deepseek", "input_cost_per_token": 5e-6, "output_cost_per_token": 6e-6}
    }"#;
    let t = table::trim(text, 0).expect("parse");
    assert_usd(t.models["x"].price.input, 3.0);
}

#[test]
fn a_missing_cache_price_takes_the_anthropic_ratio() {
    let t = table::trim(LITELLM, 42).expect("parse");
    let p = t.models["deepseek-flash"].price;
    assert_usd(p.cache_read, 0.006);
    assert_usd(p.cache_write_5m, 0.375);
    assert_usd(p.cache_write_1h, 0.6);
}

#[test]
fn a_long_tier_takes_its_threshold_from_the_key_and_skips_batches() {
    let t = table::trim(LITELLM, 42).expect("parse");
    let haiku = &t.models["claude-haiku-5-5"];
    assert_eq!(haiku.context_window, Some(1_000_000));
    assert_eq!(haiku.tiers.len(), 1);
    assert_eq!(haiku.tiers[0].over, 100_000);
    let p = haiku.tiers[0].price;
    assert_usd(p.input, 0.5);
    assert_usd(p.output, 2.5);
    assert_usd(p.cache_read, 0.05);
    assert_usd(p.cache_write_5m, 0.625);
    assert_usd(p.cache_write_1h, 1.0);
}

#[test]
fn a_cached_table_is_used_over_the_snapshot() {
    let dir = tempfile::tempdir().expect("temp dir");
    write_cache(dir.path(), &cached(5, "cached-model"));
    let t = source::load(dir.path());
    assert!(t.models.contains_key("cached-model"));
    assert!(!t.models.contains_key("claude-haiku-5-5"));
}

#[test]
fn without_a_cache_the_snapshot_is_used() {
    let dir = tempfile::tempdir().expect("temp dir");
    let t = source::load(dir.path());
    assert!(t.models.contains_key("claude-haiku-5-5"));
}

#[test]
fn an_unreadable_cache_gives_the_snapshot() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("prices.json"), "not json").expect("write");
    let t = source::load(dir.path());
    assert!(t.models.contains_key("claude-haiku-5-5"));
}

#[test]
fn a_stale_cache_is_used_when_the_fetch_fails() {
    let dir = tempfile::tempdir().expect("temp dir");
    write_cache(dir.path(), &cached(0, "cached-model"));
    let fetch = || Err(PriceError::Fetch("offline".to_owned()));
    assert!(!source::refresh_with(dir.path(), 30 * DAY_S, 7, fetch));
    assert!(source::load(dir.path()).models.contains_key("cached-model"));
    let log = std::fs::read_to_string(dir.path().join("prices.jsonl")).expect("a warning");
    assert!(log.contains("prices.refresh") && log.contains("offline"));
}

#[test]
fn a_fresh_cache_is_not_fetched() {
    let dir = tempfile::tempdir().expect("temp dir");
    write_cache(dir.path(), &cached(6 * DAY_S, "cached-model"));
    let fetch = || Err(PriceError::Fetch("must not be called".to_owned()));
    assert!(!source::refresh_with(dir.path(), 7 * DAY_S, 7, fetch));
    assert!(!dir.path().join("prices.jsonl").exists());
}

#[test]
fn a_fetched_list_replaces_the_cache() {
    let dir = tempfile::tempdir().expect("temp dir");
    let written = source::refresh_with(dir.path(), 100, 7, || Ok(LITELLM.to_owned()));
    assert!(written);
    let t = source::load(dir.path());
    assert_eq!(t.fetched_at, 100);
    assert!(t.models.contains_key("claude-haiku-5-5"));
    assert!(!dir.path().join("prices.json.tmp").exists());
}

#[test]
fn a_list_without_models_keeps_the_cache() {
    let dir = tempfile::tempdir().expect("temp dir");
    write_cache(dir.path(), &cached(0, "cached-model"));
    assert!(!source::refresh_with(dir.path(), 30 * DAY_S, 7, || Ok(
        "{}".to_owned()
    )));
    assert!(source::load(dir.path()).models.contains_key("cached-model"));
}

#[test]
fn the_cache_goes_stale_after_the_refresh_days() {
    let cache = cached(0, "cached-model");
    assert!(!source::is_stale(Some(&cache), 7 * DAY_S - 1, 7));
    assert!(source::is_stale(Some(&cache), 7 * DAY_S, 7));
    assert!(source::is_stale(None, 0, 7));
}

#[test]
fn a_refresh_period_below_one_day_counts_as_one_day() {
    let cache = cached(0, "cached-model");
    assert!(!source::is_stale(Some(&cache), DAY_S - 1, 0));
    assert!(source::is_stale(Some(&cache), DAY_S, 0));
}

#[test]
fn a_failed_fetch_holds_the_next_attempt_for_an_hour() {
    let dir = tempfile::tempdir().expect("temp dir");
    let calls = Cell::new(0);
    let fetch = || {
        calls.set(calls.get() + 1);
        Err(PriceError::Fetch("offline".to_owned()))
    };
    assert!(!source::refresh_with(dir.path(), 100, 7, fetch));
    assert!(!source::refresh_with(
        dir.path(),
        100 + HOUR_S - 1,
        7,
        fetch
    ));
    assert_eq!(calls.get(), 1);
}

#[test]
fn a_call_after_the_hour_fetches_again() {
    let dir = tempfile::tempdir().expect("temp dir");
    let calls = Cell::new(0);
    let fetch = || {
        calls.set(calls.get() + 1);
        Err(PriceError::Fetch("offline".to_owned()))
    };
    assert!(!source::refresh_with(dir.path(), 100, 7, fetch));
    assert!(!source::refresh_with(dir.path(), 100 + HOUR_S, 7, fetch));
    assert_eq!(calls.get(), 2);
}

#[test]
fn a_successful_fetch_clears_the_failure_record() {
    let dir = tempfile::tempdir().expect("temp dir");
    let failing = || Err(PriceError::Fetch("offline".to_owned()));
    assert!(!source::refresh_with(dir.path(), 100, 7, failing));
    assert!(dir.path().join("prices.attempt").exists());
    let written = source::refresh_with(dir.path(), 100 + HOUR_S, 7, || Ok(LITELLM.to_owned()));
    assert!(written);
    assert!(!dir.path().join("prices.attempt").exists());
}

#[test]
fn a_failure_recorded_by_a_later_clock_does_not_hold_the_fetch() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("prices.attempt"), "5000").expect("write the record");
    let calls = Cell::new(0);
    let fetch = || {
        calls.set(calls.get() + 1);
        Err(PriceError::Fetch("offline".to_owned()))
    };
    assert!(!source::refresh_with(dir.path(), 100, 7, fetch));
    assert_eq!(calls.get(), 1);
}

/// Writes `census/data/prices.snapshot.json` from the LiteLLM file named by `CENSUS_PRICES_SOURCE`:
/// `CENSUS_PRICES_SOURCE=<file> cargo test -p census --lib regenerates_the_snapshot -- --ignored`.
#[test]
#[ignore = "regenerates data/prices.snapshot.json; needs CENSUS_PRICES_SOURCE"]
fn regenerates_the_snapshot() {
    let source = std::env::var("CENSUS_PRICES_SOURCE").expect("set CENSUS_PRICES_SOURCE");
    let text = std::fs::read_to_string(source).expect("read the LiteLLM price file");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();
    let table = table::trim(&text, now).expect("the price file is a JSON object");
    let mut json = serde_json::to_string_pretty(&table).expect("serialize the table");
    json.push('\n');
    std::fs::write(
        concat!(env!("CARGO_MANIFEST_DIR"), "/data/prices.snapshot.json"),
        json,
    )
    .expect("write the snapshot");
}
