// SPDX-License-Identifier: MPL-2.0
//! The `census usage --compare` view: totals and roles before and after a date.

use super::{hand_back_rate, k, rate, table, usd};
use capsa_core::render::{Doc, Render};
use census::usage::{AgentStat, Report};
use serde::Serialize;

/// `census usage --compare`: the totals and each role, before and after a date, as its JSON.
#[derive(Serialize)]
pub(crate) struct CompareOut<'a> {
    pub date: &'a str,
    pub before: &'a Report,
    pub after: &'a Report,
}

impl Render for CompareOut<'_> {
    fn doc(&self, doc: &mut Doc) {
        let (b, a) = (self.before, self.after);
        let int = |n: f64| format!("{n:.0}");
        let totals = vec![
            labelled(
                "sessions",
                num_cells(b.sessions.len() as f64, a.sessions.len() as f64, int),
            ),
            labelled(
                "requests",
                num_cells(b.total.requests as f64, a.total.requests as f64, int),
            ),
            labelled("weighted", num_cells(b.weighted, a.weighted, k)),
            labelled("cost", num_cells(b.cost.usd, a.cost.usd, usd)),
            labelled(
                "cache hit",
                rate_cells(b.total.cache_hit_rate(), a.total.cache_hit_rate()),
            ),
            labelled(
                "hand-back rate",
                rate_cells(hand_back_rate(&b.handbacks), hand_back_rate(&a.handbacks)),
            ),
            labelled(
                "expired",
                num_cells(b.cache.expired as f64, a.cache.expired as f64, int),
            ),
            labelled(
                "expired cost",
                num_cells(b.cache.expired_usd, a.cache.expired_usd, usd),
            ),
            labelled(
                "busts",
                num_cells(b.cache.busts as f64, a.cache.busts as f64, int),
            ),
            labelled(
                "compactions",
                num_cells(b.compactions as f64, a.compactions as f64, int),
            ),
        ];
        let mut names: Vec<&str> = b
            .agents
            .iter()
            .chain(&a.agents)
            .map(|s| s.role.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        let mut roles: Vec<Vec<String>> = Vec::new();
        for name in names {
            let (x, y) = (role_of(b, name), role_of(a, name));
            let spawns = |s: Option<&AgentStat>| s.map_or(0.0, |s| s.spawns as f64);
            let per = |s: Option<&AgentStat>| {
                s.filter(|s| s.spawns > 0)
                    .map_or(0.0, |s| s.weighted / s.spawns as f64)
            };
            let hit = |s: Option<&AgentStat>| s.and_then(|s| s.tokens.cache_hit_rate());
            let expired = |s: Option<&AgentStat>| s.map_or(0.0, |s| s.cache.expired as f64);
            let lines = [
                ("spawns", num_cells(spawns(x), spawns(y), int)),
                ("weighted/spawn", num_cells(per(x), per(y), k)),
                ("cache hit", rate_cells(hit(x), hit(y))),
                ("expired", num_cells(expired(x), expired(y), int)),
            ];
            for (i, (metric, cells)) in lines.into_iter().enumerate() {
                let role = if i == 0 { name } else { "" };
                roles.push(
                    std::iter::once(role.to_owned())
                        .chain(std::iter::once(metric.to_owned()))
                        .chain(cells)
                        .collect(),
                );
            }
        }
        doc.title(format!("compare at {} (UTC)", self.date));
        doc.section("totals");
        doc.table(table(
            &["metric", "before", "after", "change"],
            "lrrr",
            totals,
        ));
        doc.section("roles: spawns, weighted per spawn, cache hit, expired rewrites");
        doc.table(table(
            &["role", "metric", "before", "after", "change"],
            "llrrr",
            roles,
        ));
    }
}

/// The row of `role` in a report, if it had spawns.
fn role_of<'a>(r: &'a Report, role: &str) -> Option<&'a AgentStat> {
    r.agents.iter().find(|s| s.role == role)
}

/// A table row: its name, then its cells.
fn labelled(name: &str, cells: Vec<String>) -> Vec<String> {
    std::iter::once(name.to_owned()).chain(cells).collect()
}

/// Before, after and the change in percent; `n/a` without a base.
fn num_cells(before: f64, after: f64, show: fn(f64) -> String) -> Vec<String> {
    let change = if before > 0.0 {
        format!("{:+.0}%", 100.0 * (after - before) / before)
    } else {
        "n/a".into()
    };
    vec![show(before), show(after), change]
}

/// Before, after and the change in percentage points.
fn rate_cells(before: Option<f64>, after: Option<f64>) -> Vec<String> {
    let change = match (before, after) {
        (Some(b), Some(a)) => format!("{:+.1}pp", 100.0 * (a - b)),
        _ => "n/a".into(),
    };
    vec![rate(before), rate(after), change]
}
