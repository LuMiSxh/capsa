// SPDX-License-Identifier: MPL-2.0
//! Owns census's bulletin, the status file that other tools read, and the refresh that writes it.

use super::follow::follow_rates;
use super::guard::{GuardEntry, read_log};
use capsa_core::bulletin::{self, BulletinError, Stamped};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const TOOL: &str = "census";

/// How long a written guard follow rate stays current.
pub const GUARD_FOLLOW_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// The census bulletin. A field added later is an `Option` with `#[serde(default)]`, so a bulletin
/// written by an older census still reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bulletin {
    /// The follow rate per guard rule, in 0..1, for the rules with a minimum sample.
    pub guard_follow: Stamped<BTreeMap<String, f32>>,
}

/// Writes the guard follow rates of `entries`, recorded at `now`, to the census bulletin.
///
/// # Errors
/// Fails when the bulletin cannot be encoded or written.
pub fn write_guard_follow(entries: &[GuardEntry], now: u64) -> Result<(), BulletinError> {
    let value = follow_rates(entries)
        .into_iter()
        .filter_map(|(rule, f)| f.rate().map(|r| (rule.to_owned(), r)))
        .collect();
    let stamped = Stamped {
        at: now,
        ttl: GUARD_FOLLOW_TTL_SECS,
        value,
    };
    bulletin::write(
        TOOL,
        &Bulletin {
            guard_follow: stamped,
        },
    )
}

/// Reads the whole guard log and writes its follow rates to the census bulletin.
///
/// Reads the rotated and the current log, up to their size limit, so call it off the hot path, once
/// per session at most.
///
/// # Errors
/// Fails when the bulletin cannot be encoded or written.
pub fn refresh_guard_follow(now: u64) -> Result<(), BulletinError> {
    write_guard_follow(&read_log(0, None), now)
}

/// Returns the guard follow rates while they are current at `now`, or `None` when the bulletin is
/// missing, stale or not a census bulletin.
pub fn guard_follow(now: u64) -> Option<BTreeMap<String, f32>> {
    let bulletin: Bulletin = bulletin::read(TOOL)?;
    bulletin.guard_follow.current(now).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulletin_keeps_the_guard_follow_field_name_and_stamp() {
        let b = Bulletin {
            guard_follow: Stamped {
                at: 5,
                ttl: GUARD_FOLLOW_TTL_SECS,
                value: BTreeMap::from([("grep".to_owned(), 0.5)]),
            },
        };
        let json = serde_json::to_value(&b).expect("bulletin json");
        assert_eq!(json["guard_follow"]["at"], 5);
        assert_eq!(json["guard_follow"]["ttl"], 604_800);
        assert_eq!(json["guard_follow"]["value"]["grep"], 0.5);
    }
}
