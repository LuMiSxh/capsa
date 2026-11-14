// SPDX-License-Identifier: MPL-2.0
//! Reads the capsa bulletins of vicarius and acus, and the release bulletins of the capsa tools,
//! which the status line shows as segments.

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

/// Returns the interop segments for the status line of `session`: the DeepSeek gate, the guard
/// blocks and the update segment, from the bulletins that are current at `now`. A missing, stale or
/// unreadable bulletin gives no segment.
pub fn segments(session: Option<&str>, now: u64) -> Vec<String> {
    let segments = deepseek_segment(now)
        .into_iter()
        .chain(session.and_then(|s| guard_segment(s, now)));
    #[cfg(feature = "cmd-update")]
    let segments = segments.chain(update_segment(capsa_core::update::release_status));
    segments.collect()
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

/// The capsa tools whose release bulletins the status line checks for an update.
#[cfg(feature = "cmd-update")]
const RELEASE_TOOLS: [&str; 4] = ["acus", "census", "pons", "vicarius"];

/// Names the tools that `latest` finds a newer release for, such as `update acus,pons`, or `None`
/// when no tool has one.
#[cfg(feature = "cmd-update")]
fn update_segment(latest: impl Fn(&str) -> Option<String>) -> Option<String> {
    let tools: Vec<&str> = RELEASE_TOOLS
        .into_iter()
        .filter(|tool| latest(tool).is_some())
        .collect();
    (!tools.is_empty()).then(|| format!("update {}", tools.join(",")))
}

#[cfg(all(test, feature = "cmd-update"))]
mod tests {
    use super::update_segment;

    #[test]
    fn no_update_segment_when_no_tool_has_a_newer_release() {
        assert_eq!(update_segment(|_| None), None);
    }

    #[test]
    fn update_segment_names_the_tools_with_a_newer_release_in_tool_order() {
        let newer = |tool: &str| matches!(tool, "pons" | "acus").then(|| "0.24.0".to_owned());
        assert_eq!(update_segment(newer).as_deref(), Some("update acus,pons"));
    }
}
