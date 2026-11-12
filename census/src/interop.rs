// SPDX-License-Identifier: MPL-2.0
//! Reads the capsa bulletins of vicarius and acus, which the status line shows as segments.

use capsa_core::bulletin::{self, Stamped};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The part of the vicarius bulletin that the status line reads. Other fields are ignored, so
/// vicarius can add some.
#[derive(Deserialize)]
struct Vicarius {
    deepseek: Stamped<DeepSeek>,
}

/// The DeepSeek gate: whether `-ds` subagents are routed to DeepSeek, and the balance left.
#[derive(Deserialize)]
struct DeepSeek {
    open: bool,
    balance_usd: Option<f64>,
}

/// The part of the acus bulletin that the status line reads. Other fields are ignored, so acus can
/// add some.
#[derive(Deserialize)]
struct Acus {
    guard_blocks: Stamped<BTreeMap<String, u32>>,
}

/// Returns the interop segments for the status line of `session`, the DeepSeek gate first, from the
/// bulletins that are current at `now`. A missing, stale or unreadable bulletin gives no segment.
pub fn segments(session: Option<&str>, now: u64) -> Vec<String> {
    deepseek_segment(now)
        .into_iter()
        .chain(session.and_then(|s| guard_segment(s, now)))
        .collect()
}

fn deepseek_segment(now: u64) -> Option<String> {
    let vicarius: Vicarius = bulletin::read("vicarius")?;
    let gate = vicarius.deepseek.current(now)?;
    Some(match (gate.open, gate.balance_usd) {
        (false, _) => "ds off".to_owned(),
        (true, Some(usd)) => format!("ds ${usd:.2}"),
        (true, None) => "ds open".to_owned(),
    })
}

fn guard_segment(session: &str, now: u64) -> Option<String> {
    let acus: Acus = bulletin::read("acus")?;
    let blocks = *acus.guard_blocks.current(now)?.get(session)?;
    (blocks > 0).then(|| format!("guard {blocks}"))
}
