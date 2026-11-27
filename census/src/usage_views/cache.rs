// SPDX-License-Identifier: MPL-2.0
//! The `census usage --cache` view: the pauses between requests and the cost of each context size.

use super::{k, money, table, usd};
use capsa_core::render::{Doc, Render};
use census::usage::{ContextBucket, GapBucket};
use serde::Serialize;

/// `census usage --cache`: requests by the pause after the previous one, then by context size.
#[derive(Serialize)]
pub(crate) struct CacheView<'a> {
    gaps: &'a [GapBucket],
    contexts: &'a [ContextBucket],
}

impl<'a> CacheView<'a> {
    pub(crate) fn new(gaps: &'a [GapBucket], contexts: &'a [ContextBucket]) -> Self {
        Self { gaps, contexts }
    }

    fn gaps_table(&self, doc: &mut Doc) {
        doc.section("cache gaps");
        let rows = self
            .gaps
            .iter()
            .map(|g| {
                vec![
                    g.label.to_owned(),
                    g.requests.to_string(),
                    k(g.cache_read as f64),
                    k(g.cache_writes as f64),
                    g.rewrites.to_string(),
                    usd(g.rewrite_usd),
                    k(g.output as f64),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "pause",
                "requests",
                "cache read",
                "cache writes",
                "rewrites",
                "rewrite cost",
                "output",
            ],
            "lrrrrrr",
            rows,
        ));
        doc.line(
            "a rewrite is a cache write over 5k tokens after a pause over 5 min; its cost is the \
             list price of those writes",
        );
    }

    fn contexts_table(&self, doc: &mut Doc) {
        doc.section("cost by context size");
        let rows = self
            .contexts
            .iter()
            .map(|c| {
                vec![
                    c.role.to_owned(),
                    c.label.to_owned(),
                    c.requests.to_string(),
                    money(&c.cost),
                    k(c.cache_read as f64),
                    k(c.cache_writes as f64),
                    k(c.output as f64),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "role",
                "context",
                "requests",
                "cost",
                "cache read",
                "cache writes",
                "output",
            ],
            "llrrrrr",
            rows,
        ));
        let main: u64 = self
            .contexts
            .iter()
            .filter(|c| c.role == "main")
            .map(|c| c.requests)
            .sum();
        let over: u64 = self
            .contexts
            .iter()
            .filter(|c| c.role == "main" && matches!(c.label, "200-300k" | ">300k"))
            .map(|c| c.requests)
            .sum();
        if main > 0 {
            doc.line(format!(
                "main requests over 200k: {over} of {main} ({:.1}%)",
                100.0 * over as f64 / main as f64
            ));
        }
    }
}

impl Render for CacheView<'_> {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --cache");
        self.gaps_table(doc);
        self.contexts_table(doc);
    }
}
