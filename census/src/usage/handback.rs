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

/// The spawns and hand-backs of one role. A hand-back is counted against the role of the worker
/// spawn that it followed.
#[derive(Debug, Default, Clone)]
pub struct RoleHandbacks {
    pub role: String,
    pub spawns: u64,
    pub handbacks: u64,
    /// Median minutes from a handed-back spawn's start to the senior-worker spawn that took it over.
    /// `None` without hand-backs.
    pub median_min: Option<f64>,
}

/// Counts the spawns per ISO week. Each pair is a spawn with the session it belongs to.
pub(super) fn weeks(spawns: &[(&str, &SpawnCall)]) -> Vec<HandbackWeek> {
    let mut weeks: BTreeMap<String, HandbackWeek> = BTreeMap::new();
    for calls in by_session(spawns).values() {
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
                    if source(calls, c).is_some() {
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

/// The worker spawn of `calls` that `senior` took over: the latest one that `follows`.
fn source<'a>(calls: &[&'a SpawnCall], senior: &SpawnCall) -> Option<&'a SpawnCall> {
    calls
        .iter()
        .copied()
        .filter(|h| h.role == "worker" && follows(h, senior))
        .max_by_key(|h| h.start_ms)
}

/// Groups the spawns by the session they belong to.
fn by_session<'s, 'c>(spawns: &[(&'s str, &'c SpawnCall)]) -> HashMap<&'s str, Vec<&'c SpawnCall>> {
    let mut sessions: HashMap<&str, Vec<&SpawnCall>> = HashMap::new();
    for &(session, c) in spawns {
        sessions.entry(session).or_default().push(c);
    }
    sessions
}

/// Counts the spawns and hand-backs of each role. A hand-back is timed from the start of the worker
/// spawn it followed to the start of the senior-worker spawn.
pub(super) fn roles(spawns: &[(&str, &SpawnCall)]) -> Vec<RoleHandbacks> {
    let mut spawned: BTreeMap<&str, u64> = BTreeMap::new();
    let mut minutes: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for calls in by_session(spawns).values() {
        for &c in calls {
            *spawned.entry(c.role.as_str()).or_default() += 1;
            if c.role != "senior-worker" {
                continue;
            }
            if let Some(h) = source(calls, c) {
                let gap = c.start_ms.saturating_sub(h.start_ms);
                minutes
                    .entry(h.role.as_str())
                    .or_default()
                    .push(gap as f64 / 60_000.0);
            }
        }
    }
    spawned
        .into_iter()
        .map(|(role, count)| {
            let times = minutes.get(role).cloned().unwrap_or_default();
            RoleHandbacks {
                role: role.to_owned(),
                spawns: count,
                handbacks: times.len() as u64,
                median_min: median(times),
            }
        })
        .collect()
}

/// The median of `xs`, or `None` when it is empty.
fn median(mut xs: Vec<f64>) -> Option<f64> {
    xs.sort_by(f64::total_cmp);
    let n = xs.len();
    let upper = *xs.get(n / 2)?;
    if n % 2 == 1 {
        return Some(upper);
    }
    let lower = *xs.get(n / 2 - 1)?;
    Some((lower + upper) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::SpawnCall;

    fn spawn(role: &str, start_ms: u64, end_ms: Option<u64>) -> SpawnCall {
        SpawnCall {
            id: String::new(),
            role: role.to_owned(),
            start_ms,
            end_ms,
        }
    }

    #[test]
    fn roles_time_each_hand_back_from_the_worker_spawn_it_followed() {
        let (w1, s1) = (
            spawn("worker", 0, Some(60_000)),
            spawn("senior-worker", 120_000, None),
        );
        let (w2, s2) = (
            spawn("worker", 1_000_000, Some(1_000_000)),
            spawn("senior-worker", 1_000_000 + 6 * 60_000, None),
        );
        let w3 = spawn("worker-lite", 0, None);
        let spawns = [("a", &w1), ("a", &s1), ("b", &w2), ("b", &s2), ("c", &w3)];
        let rows = roles(&spawns);
        let row = |name: &str| rows.iter().find(|r| r.role == name).expect("role row");
        assert_eq!(row("worker").spawns, 2);
        assert_eq!(row("worker").handbacks, 2);
        assert_eq!(row("worker").median_min, Some(4.0));
        assert_eq!(row("senior-worker").handbacks, 0);
        assert_eq!(row("senior-worker").median_min, None);
        assert_eq!(row("worker-lite").spawns, 1);
    }

    #[test]
    fn median_takes_the_middle_value_or_the_mean_of_the_two_middle_ones() {
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 9.0]), Some(3.0));
        assert_eq!(median(Vec::new()), None);
    }
}
