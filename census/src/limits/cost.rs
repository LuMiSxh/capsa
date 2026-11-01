// SPDX-License-Identifier: MPL-2.0
//! Prices the ledger at list price, so that windows and hours can be summed in dollars.

use super::clock::hour_start;
use crate::ledger::{Hourly, Row};
use crate::usage::Tokens;
use crate::usage::prices;
use std::collections::BTreeMap;

/// Holds the list-price USD of each raw request and each hourly aggregate, each list sorted by
/// time.
pub(super) struct Costed {
    /// Request time and USD of each raw row.
    raw: Vec<(u64, f64)>,
    /// Hour start and USD of each hourly aggregate.
    hourly: Vec<(u64, f64)>,
}

impl Costed {
    /// Prices the rows and the aggregates. A model without a price counts as 0 USD.
    pub(super) fn new(rows: &[Row], hourly: &[Hourly]) -> Self {
        let mut raw: Vec<(u64, f64)> = rows.iter().map(|r| (r.t, row_usd(r))).collect();
        raw.sort_by_key(|x| x.0);
        let mut agg: Vec<(u64, f64)> = hourly.iter().map(|h| (h.hour, aggregate_usd(h))).collect();
        agg.sort_by_key(|x| x.0);
        Costed { raw, hourly: agg }
    }

    /// Returns the USD of the requests from `from` to `to` inclusive, plus the hourly aggregates
    /// whose hour starts from `from` up to `to`.
    pub(super) fn between(&self, from: u64, to: u64) -> f64 {
        sum(&self.raw, from, to.saturating_add(1)) + sum(&self.hourly, from, to)
    }

    /// Returns the time of the first raw request at or after `t`.
    pub(super) fn first_request_from(&self, t: u64) -> Option<u64> {
        self.raw
            .get(self.raw.partition_point(|x| x.0 < t))
            .map(|x| x.0)
    }

    /// Returns the start of the first hour with any record.
    pub(super) fn first_hour(&self) -> Option<u64> {
        [
            self.raw.first().map(|x| hour_start(x.0)),
            self.hourly.first().map(|x| x.0),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Returns the USD of each hour, keyed by its start, from raw requests and aggregates.
    pub(super) fn hour_costs(&self) -> BTreeMap<u64, f64> {
        let mut out = BTreeMap::new();
        for &(t, usd) in &self.raw {
            *out.entry(hour_start(t)).or_insert(0.0) += usd;
        }
        for &(hour, usd) in &self.hourly {
            *out.entry(hour).or_insert(0.0) += usd;
        }
        out
    }
}

/// Sums the USD of the entries of a list sorted by time whose time lies in `from..to`.
fn sum(v: &[(u64, f64)], from: u64, to: u64) -> f64 {
    let a = v.partition_point(|x| x.0 < from);
    let b = v.partition_point(|x| x.0 < to);
    v.get(a..b).map_or(0.0, |s| s.iter().map(|x| x.1).sum())
}

fn row_usd(r: &Row) -> f64 {
    let t = Tokens {
        input: r.input,
        cache_read: r.cache_read,
        cache_write_5m: r.cache_write_5m,
        cache_write_1h: r.cache_write_1h,
        output: r.output,
        requests: 1,
    };
    prices::request_cost(&r.model, &t).unwrap_or(0.0)
}

fn aggregate_usd(h: &Hourly) -> f64 {
    let t = &h.tokens;
    let t = Tokens {
        input: t.input,
        cache_read: t.cache_read,
        cache_write_5m: t.cache_write_5m,
        cache_write_1h: t.cache_write_1h,
        output: t.output,
        requests: t.requests,
    };
    prices::family_cost(&h.family, &t).unwrap_or(0.0)
}
