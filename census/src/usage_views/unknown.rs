// SPDX-License-Identifier: MPL-2.0
//! The `census usage --unknown` view: the models the price list cannot price.

use super::{k, table};
use capsa_core::render::{Doc, Render};
use census::usage::ModelStat;
use serde::Serialize;

/// One unpriced model, as the table's JSON.
#[derive(Serialize)]
struct UnknownRow {
    model: String,
    provider: String,
    requests: u64,
    weighted: f64,
}

/// `census usage --unknown`: the models without a list price, most weighted tokens first.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct UnknownView(Vec<UnknownRow>);

impl UnknownView {
    pub(crate) fn new(models: &[ModelStat]) -> Self {
        Self(
            models
                .iter()
                .filter(|m| m.cost.unpriced > 0)
                .map(|m| UnknownRow {
                    model: m.model.clone(),
                    provider: m.provider.clone(),
                    requests: m.tokens.requests,
                    weighted: m.tokens.weighted(),
                })
                .collect(),
        )
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Render for UnknownView {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --unknown");
        if self.0.is_empty() {
            doc.line("no unpriced requests: every model has a list price");
            return;
        }
        let rows = self
            .0
            .iter()
            .map(|m| {
                vec![
                    m.model.clone(),
                    m.provider.clone(),
                    m.requests.to_string(),
                    k(m.weighted),
                ]
            })
            .collect();
        doc.table(table(
            &["model", "provider", "requests", "weighted"],
            "llrr",
            rows,
        ));
        doc.line(
            "a model without a list price costs 0 USD; name its provider with `[provider_prefixes]` \
             in the config",
        );
    }
}
