// SPDX-License-Identifier: MPL-2.0
//! Owns the hand-back heuristic: a senior-worker spawn soon after a worker spawn of its session.

use super::SpawnCall;
use super::time::iso_week;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// A senior-worker spawn starting within this long after a worker spawn finished is a hand-back.
const WINDOW_MS: u64 = 30 * 60 * 1000;

/// The spawns of one ISO week, by start time. `rate` is hand-backs per worker spawn.
#[derive(Debug, Default, Clone, Serialize)]
pub struct HandbackWeek {
    pub week: String,
    pub workers: u64,
    pub seniors: u64,
    pub handbacks: u64,
    /// `None` in a week without worker spawns.
    pub rate: Option<f64>,
}

/// Counts the spawns per ISO week. Each pair is a spawn with the session it belongs to.
pub(super) fn weeks(spawns: &[(&str, &SpawnCall)]) -> Vec<HandbackWeek> {
    let mut sessions: HashMap<&str, Vec<&SpawnCall>> = HashMap::new();
    for &(session, c) in spawns {
        sessions.entry(session).or_default().push(c);
    }
    let mut weeks: BTreeMap<String, HandbackWeek> = BTreeMap::new();
    for calls in sessions.values() {
        for &c in calls {
            let key = iso_week(c.start_ms);
            let w = weeks.entry(key.clone()).or_insert_with(|| HandbackWeek {
                week: key,
                ..Default::default()
            });
            match c.role.as_str() {
                "worker" => w.workers += 1,
                "senior-worker" => {
                    w.seniors += 1;
                    if calls.iter().any(|&h| h.role == "worker" && follows(h, c)) {
                        w.handbacks += 1;
                    }
                }
                _ => {}
            }
        }
    }
    weeks
        .into_values()
        .map(|mut w| {
            w.rate = (w.workers > 0).then(|| w.handbacks as f64 / w.workers as f64);
            w
        })
        .collect()
}

/// Whether `senior` started within the window after `worker` finished, or started if its end is
/// unknown.
fn follows(worker: &SpawnCall, senior: &SpawnCall) -> bool {
    let done = worker.end_ms.unwrap_or(worker.start_ms);
    senior.start_ms >= done && senior.start_ms - done <= WINDOW_MS
}
