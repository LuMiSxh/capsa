// SPDX-License-Identifier: MPL-2.0
//! The `census usage --providers` view: requests, tokens, cost and billing per provider.

use super::{k, table, usd};
use capsa_core::render::{Doc, Render};
use census::config::Config;
use census::usage::ProviderStat;
use serde::Serialize;

/// One provider of the report, as the table's JSON.
#[derive(Serialize)]
struct ProviderRow {
    provider: String,
    requests: u64,
    weighted: f64,
    cost_usd: f64,
    unpriced: u64,
    billing: &'static str,
}

/// `census usage --providers`: one row per provider, sorted by name.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct ProvidersView(Vec<ProviderRow>);

impl ProvidersView {
    pub(crate) fn new(providers: &[ProviderStat], config: &Config) -> Self {
        Self(
            providers
                .iter()
                .map(|p| ProviderRow {
                    provider: p.provider.clone(),
                    requests: p.tokens.requests,
                    weighted: p.tokens.weighted(),
                    cost_usd: p.cost.usd,
                    unpriced: p.cost.unpriced,
                    billing: config.billing_of(&p.provider).name(),
                })
                .collect(),
        )
    }
}

impl Render for ProvidersView {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --providers");
        let rows = self
            .0
            .iter()
            .map(|p| {
                vec![
                    p.provider.clone(),
                    p.billing.to_owned(),
                    p.requests.to_string(),
                    k(p.weighted),
                    usd(p.cost_usd),
                    p.unpriced.to_string(),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "provider", "billing", "requests", "weighted", "cost", "unpriced",
            ],
            "llrrrr",
            rows,
        ));
    }
}
