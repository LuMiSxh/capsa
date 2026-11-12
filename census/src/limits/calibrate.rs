// SPDX-License-Identifier: MPL-2.0
//! Calibrates a limit window: how many percent of it one list-price dollar uses, as the median
//! over the recent snapshots.

use super::clock::DAY_S;
use super::cost::Costed;
use super::window;
use capsa_core::census_snapshot::{Snapshot, Window};
use serde::Serialize;

/// Points behind the median: the most recent ones.
const POINTS: usize = 20;
/// Snapshots older than this give no point, unless the recent ones give too few points.
const MAX_AGE_S: u64 = 30 * DAY_S;
/// The window that replaces `MAX_AGE_S` while the recent snapshots give too few points.
const WIDE_AGE_S: u64 = 90 * DAY_S;
/// A window with less list-price cost than this gives no point: its ratio is noise.
const MIN_COST_USD: f64 = 0.01;
/// The spread needs this many points for its quartiles.
const MIN_SPREAD_POINTS: usize = 4;

/// Holds the calibration of one window type.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Calibration {
    /// Percent of the window per list-price USD: the median of the points.
    pub k: f64,
    /// Interquartile range of the points over `k`; `None` below four points.
    pub spread: Option<f64>,
    /// Points behind `k`.
    pub points: usize,
}

/// Returns the calibration of `window` from its snapshots, or `None` when none gives a point.
///
/// A point is a snapshot of the window with a share above zero, a reset after its time, and a
/// list-price cost of the window from its start to the snapshot of at least $0.01. The snapshots
/// are no older than 30 days; while they give fewer than four points, 90 days.
pub(super) fn calibrate(
    window: Window,
    snapshots: &[Snapshot],
    costs: &Costed,
    now: u64,
) -> Option<Calibration> {
    let mut ks = points(window, snapshots, costs, now, MAX_AGE_S);
    if ks.len() < MIN_SPREAD_POINTS {
        ks = points(window, snapshots, costs, now, WIDE_AGE_S);
    }
    if ks.is_empty() {
        return None;
    }
    let points = ks.len();
    ks.sort_by(f64::total_cmp);
    let k = quantile(&ks, 0.5);
    let spread =
        (points >= MIN_SPREAD_POINTS).then(|| (quantile(&ks, 0.75) - quantile(&ks, 0.25)) / k);
    Some(Calibration { k, spread, points })
}

/// Returns the share per USD of the most recent snapshots of `window` no older than `max_age_s`.
fn points(
    window: Window,
    snapshots: &[Snapshot],
    costs: &Costed,
    now: u64,
    max_age_s: u64,
) -> Vec<f64> {
    let len = window::length(window);
    let mut recent: Vec<&Snapshot> = snapshots
        .iter()
        .filter(|s| {
            s.window == window
                && now.saturating_sub(s.t) <= max_age_s
                && s.resets_at > s.t
                && s.used_percentage > 0.0
        })
        .collect();
    recent.sort_by_key(|s| s.t);
    recent
        .iter()
        .rev()
        .filter_map(|s| {
            let cost = costs.between(s.resets_at.saturating_sub(len), s.t);
            (cost >= MIN_COST_USD).then(|| s.used_percentage / cost)
        })
        .take(POINTS)
        .collect()
}

/// Returns the `p` quantile of sorted values, interpolated between the two nearest.
pub(super) fn quantile(sorted: &[f64], p: f64) -> f64 {
    let pos = p * sorted.len().saturating_sub(1) as f64;
    let at = |i: f64| sorted.get(i as usize).copied().unwrap_or(0.0);
    let lo = pos.floor();
    at(lo) + (at(pos.ceil()) - at(lo)) * (pos - lo)
}

/// Returns the median of the last `n` values, or `None` without a value.
pub(super) fn typical(values: &[f64], n: usize) -> Option<f64> {
    let mut last: Vec<f64> = values.iter().rev().take(n).copied().collect();
    last.sort_by(f64::total_cmp);
    (!last.is_empty()).then(|| quantile(&last, 0.5))
}
