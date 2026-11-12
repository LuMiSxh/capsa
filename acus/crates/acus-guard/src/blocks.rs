// SPDX-License-Identifier: MPL-2.0
//! Counts the guard blocks of each session and publishes the latest sessions' counts in the capsa
//! bulletin, which census reads for its statusline.

use crate::GuardConfig;
use crate::hints::{self, BLOCKS};
use capsa_core::bulletin::{self, Stamped};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const TOOL: &str = "acus";
/// Sets how long the published counts stay current. Each block and session start writes them again.
const TTL_SECS: u64 = 24 * 60 * 60;
/// Sets how many sessions the bulletin keeps.
const MAX_SESSIONS: usize = 16;

/// The acus bulletin: the block count of the latest sessions, keyed by session id.
#[derive(Serialize, Deserialize)]
struct Bulletin {
    guard_blocks: Stamped<BTreeMap<String, u32>>,
}

/// Counts one more block of `session` and publishes the count. A session without an id is not
/// counted.
pub fn record(cfg: &GuardConfig, session: &str) {
    let now = bulletin::now();
    let Some(count) = hints::edit_session(cfg, session, |counts| {
        let blocks = counts.entry(BLOCKS.to_owned()).or_default();
        blocks.n += 1;
        blocks.last = now;
        blocks.n
    }) else {
        return;
    };
    publish(cfg, session, count, now);
}

/// Publishes `session` at its start with its block count, or 0 when it has none. The start counts
/// as the session's last activity, which decides which session the bulletin drops first.
pub fn register(cfg: &GuardConfig, session: &str) {
    let now = bulletin::now();
    let Some(count) = hints::edit_session(cfg, session, |counts| {
        let blocks = counts.entry(BLOCKS.to_owned()).or_default();
        blocks.last = now;
        blocks.n
    }) else {
        return;
    };
    publish(cfg, session, count, now);
}

/// Sets the count of `session` in the bulletin, then drops the sessions past `MAX_SESSIONS`. The
/// read and the write hold a lock, so two hooks running at once do not drop each other's counts.
fn publish(cfg: &GuardConfig, session: &str, count: u64, now: u64) {
    let _lock = hints::acquire(&cfg.state_dir.join(".bulletin"));
    let mut sessions = bulletin::read::<Bulletin>(TOOL)
        .map(|b| b.guard_blocks.value)
        .unwrap_or_default();
    sessions.insert(session.to_owned(), u32::try_from(count).unwrap_or(u32::MAX));
    trim(&mut sessions, session, MAX_SESSIONS, |s| {
        last_activity(cfg, s)
    });
    let bulletin = Bulletin {
        guard_blocks: Stamped {
            at: now,
            ttl: TTL_SECS,
            value: sessions,
        },
    };
    // The bulletin only feeds a status line, so a write that fails keeps the previous counts.
    let _ = bulletin::write(TOOL, &bulletin);
}

/// Returns the unix time of the last block or start of `session`, or 0 when its state is unknown.
fn last_activity(cfg: &GuardConfig, session: &str) -> u64 {
    hints::state_file(cfg, session)
        .and_then(|path| hints::read(&path, 1).ok())
        .and_then(|counts| counts.get(BLOCKS).map(|c| c.last))
        .unwrap_or(0)
}

/// Drops the session with the oldest activity until `max` remain. The session being written is
/// never dropped.
fn trim(
    sessions: &mut BTreeMap<String, u32>,
    keep: &str,
    max: usize,
    last_activity: impl Fn(&str) -> u64,
) {
    while sessions.len() > max {
        let oldest = sessions
            .keys()
            .filter(|s| s.as_str() != keep)
            .map(|s| (last_activity(s), s.clone()))
            .min();
        let Some((_, oldest)) = oldest else {
            break;
        };
        sessions.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trimmed(sessions: &[(&str, u32)], keep: &str, activity: &[(&str, u64)]) -> Vec<String> {
        let mut map: BTreeMap<String, u32> = sessions
            .iter()
            .map(|(s, n)| ((*s).to_owned(), *n))
            .collect();
        trim(&mut map, keep, 2, |s| {
            activity
                .iter()
                .find(|(k, _)| *k == s)
                .map_or(0, |(_, t)| *t)
        });
        map.into_keys().collect()
    }

    #[test]
    fn trim_drops_the_session_with_the_oldest_activity() {
        let kept = trimmed(
            &[("a", 1), ("b", 2), ("c", 3)],
            "c",
            &[("a", 5), ("b", 1), ("c", 3)],
        );
        assert_eq!(kept, ["a", "c"]);
    }

    #[test]
    fn trim_never_drops_the_session_being_written() {
        let kept = trimmed(
            &[("a", 1), ("b", 2), ("c", 3)],
            "a",
            &[("a", 0), ("b", 5), ("c", 6)],
        );
        assert_eq!(kept, ["a", "c"]);
    }

    #[test]
    fn trim_leaves_a_bulletin_within_the_limit_alone() {
        let kept = trimmed(&[("a", 1), ("b", 2)], "a", &[("a", 0), ("b", 0)]);
        assert_eq!(kept, ["a", "b"]);
    }
}
