// SPDX-License-Identifier: MPL-2.0
//! The tables of `census usage`, one module per view, and the helpers that they share.

use capsa_core::render::{Align, Doc, Table};
use census::config::{Billing, Config};
use census::usage::{Cost, HandbackWeek, Report};

mod agents;
mod by;
mod compare;
mod handbacks;
mod tools;

pub(crate) use agents::AgentsView;
pub(crate) use by::ByView;
pub(crate) use compare::CompareOut;
pub(crate) use handbacks::HandbacksView;
pub(crate) use tools::ToolsView;

/// A table with one header per column; `align` holds an `r` for each right-aligned column.
pub(crate) fn table(head: &[&str], align: &str, rows: Vec<Vec<String>>) -> Table {
    Table {
        head: head.iter().map(|h| (*h).to_owned()).collect(),
        align: align
            .chars()
            .map(|c| if c == 'r' { Align::Right } else { Align::Left })
            .collect(),
        rows,
    }
}

/// 1234567 → "1.2M".
pub(crate) fn k(n: f64) -> String {
    match n {
        n if n >= 1e9 => format!("{:.1}G", n / 1e9),
        n if n >= 1e6 => format!("{:.1}M", n / 1e6),
        n if n >= 1e3 => format!("{:.0}k", n / 1e3),
        n => format!("{n:.0}"),
    }
}

/// `$1.23`. Adding zero turns the `-0.0` of an empty sum into `0.0`, which would print as `$-0.00`.
pub(crate) fn usd(x: f64) -> String {
    format!("${:.2}", x + 0.0)
}

/// `$1.23`, `n/a` when no request has a price, and `(+N unpriced)` beside a partial price.
pub(crate) fn money(c: &Cost) -> String {
    match c.unpriced {
        0 => usd(c.usd),
        _ if c.usd <= 0.0 => "n/a".into(),
        n => format!("{} (+{n} unpriced)", usd(c.usd)),
    }
}

/// `93.1%`, or `n/a` without a value.
pub(crate) fn rate(x: Option<f64>) -> String {
    x.map_or_else(|| "n/a".into(), |x| format!("{:.1}%", 100.0 * x))
}

/// The context sizes before a session's compactions, as `180k,224k`; empty without compactions.
pub(crate) fn sizes_text(sizes: &[u64]) -> String {
    sizes
        .iter()
        .map(|&n| k(n as f64))
        .collect::<Vec<_>>()
        .join(",")
}

/// Names what a provider's requests cost the user, as `billing` in the config sets it. An API
/// provider that the price list does not know is `not in limits`.
pub(crate) fn provider_kind(config: &Config, provider: &str) -> &'static str {
    match (config.billing_of(provider), provider) {
        (Billing::Subscription, _) => "subscription",
        (Billing::Api, census::UNKNOWN) => "not in limits",
        (Billing::Api, _) => "API",
    }
}

/// The cost, cache and compaction rows of the summary.
pub(crate) fn cost_rows(doc: &mut Doc, r: &Report, config: &Config) {
    doc.row("cost").text(format!(
        "{} at list prices, unpriced requests: {}",
        usd(r.cost.usd),
        r.cost.unpriced
    ));
    let providers: Vec<String> = r
        .providers
        .iter()
        .map(|p| {
            format!(
                "{} {} ({})",
                p.provider,
                usd(p.cost.usd),
                provider_kind(config, p.provider)
            )
        })
        .collect();
    doc.row("providers").text(providers.join(", "));
    doc.row("cache").text(format!(
        "hit {}, expired-cache rewrites {} ({}), unexpected busts {}",
        rate(r.total.cache_hit_rate()),
        r.cache.expired,
        usd(r.cache.expired_usd),
        r.cache.busts
    ));
    doc.row("compactions").text(format!(
        "{} in {} sessions, context before them p50 {} p90 {}",
        r.compactions,
        r.sessions.iter().filter(|s| s.compactions > 0).count(),
        k(r.compaction_context[0] as f64),
        k(r.compaction_context[1] as f64)
    ));
}

/// Hand-backs per worker spawn over all weeks, or `None` without worker spawns.
fn hand_back_rate(rows: &[HandbackWeek]) -> Option<f64> {
    let workers: u64 = rows.iter().map(|w| w.workers).sum();
    let handbacks: u64 = rows.iter().map(|w| w.handbacks).sum();
    (workers > 0).then(|| handbacks as f64 / workers as f64)
}
