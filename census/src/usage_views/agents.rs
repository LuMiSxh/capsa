// SPDX-License-Identifier: MPL-2.0
//! The `census usage --agents` view: one row per role.

use super::{k, money, rate, table, usd};
use capsa_core::render::{Doc, Render};
use census::usage::AgentStat;
use serde::Serialize;

/// `census usage --agents`: one row per role, as its JSON. Codex transcripts carry no roles and
/// count under `main`.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct AgentsView<'a>(pub &'a [AgentStat]);

impl Render for AgentsView<'_> {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --agents");
        let rows = self
            .0
            .iter()
            .map(|a| {
                vec![
                    a.role.clone(),
                    a.spawns.to_string(),
                    a.models.join(","),
                    format!("{:.0}", a.median_requests),
                    k(a.median_first_context),
                    k(a.weighted),
                    format!("{:.1}%", 100.0 * a.share),
                    k(a.median_weighted),
                    money(&a.cost),
                    rate(a.tokens.cache_hit_rate()),
                    a.cache.expired.to_string(),
                    usd(a.cache.expired_usd),
                    a.cache.busts.to_string(),
                    a.cache.compactions.to_string(),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "role",
                "spawns",
                "models",
                "median-reqs",
                "median-first-ctx",
                "weighted",
                "share",
                "median-weighted/spawn",
                "cost",
                "cache-hit",
                "expired",
                "expired-cost",
                "busts",
                "compactions",
            ],
            "lrlrrrrrrrrrrr",
            rows,
        ));
    }
}
