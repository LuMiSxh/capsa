// SPDX-License-Identifier: MPL-2.0
//! Summarises the subagent spawns of a report by role: medians, models, cost and cache.

use crate::usage::{AgentStat, CacheStat, Cost, Tokens};
use std::collections::HashMap;

/// One transcript's requests after dedup.
#[derive(Default)]
pub(super) struct Spawn {
    pub(super) requests: u64,
    first_context: u64,
    weighted: f64,
    models: HashMap<String, u64>,
    tokens: Tokens,
    cost: Cost,
    pub(super) cache: CacheStat,
}

impl Spawn {
    pub(super) fn add(&mut self, model: &str, t: &Tokens, cost: &Cost) {
        if self.requests == 0 {
            self.first_context = t.context();
        }
        self.requests += 1;
        self.weighted += t.weighted();
        self.tokens.add(t);
        self.cost.merge(cost);
        if !model.is_empty() && model != "<synthetic>" {
            *self.models.entry(model.to_owned()).or_default() += 1;
        }
    }
}

pub(super) fn agent_stats(roles: HashMap<String, Vec<Spawn>>, total: f64) -> Vec<AgentStat> {
    let mut out: Vec<AgentStat> = roles
        .into_iter()
        .map(|(role, spawns)| {
            let col = |f: fn(&Spawn) -> f64| median(spawns.iter().map(f).collect());
            let mut models: HashMap<&str, u64> = HashMap::new();
            for (m, n) in spawns.iter().flat_map(|s| &s.models) {
                *models.entry(m).or_default() += n;
            }
            let mut models: Vec<_> = models.into_iter().collect();
            models.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let weighted = spawns.iter().map(|s| s.weighted).sum::<f64>();
            let mut tokens = Tokens::default();
            let mut cost = Cost::default();
            let mut cache = CacheStat::default();
            for s in &spawns {
                tokens.add(&s.tokens);
                cost.merge(&s.cost);
                cache.add(&s.cache);
            }
            AgentStat {
                spawns: spawns.len() as u64,
                models: models.into_iter().map(|(m, _)| m.to_owned()).collect(),
                median_requests: col(|s| s.requests as f64),
                median_first_context: col(|s| s.first_context as f64),
                weighted,
                share: weighted / total.max(1.0),
                median_weighted: col(|s| s.weighted),
                tokens,
                cost,
                cache,
                role,
            }
        })
        .collect();
    out.sort_by(|a, b| b.weighted.total_cmp(&a.weighted).then(a.role.cmp(&b.role)));
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}
