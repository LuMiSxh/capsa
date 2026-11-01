// SPDX-License-Identifier: MPL-2.0
//! Forecasts the weekly window from its share now and the expected cost until its reset, and
//! summarises the five-hour windows of the day.

use super::calibrate::typical;
use super::clock::{DAY_S, HOUR_S, WEEKDAYS, day_start, hour_start, weekday};
use super::cost::Costed;
use super::pattern::{Expect, HALF_LIFE_S};
use super::window::{Span, five_hour_starts, length};
use capsa_core::census_snapshot::{Snapshot, Window};
use serde::Serialize;
use std::collections::BTreeMap;

/// The first hours after now also follow the current rate.
const RATE_S: u64 = 2 * HOUR_S;
/// The share of the current rate in those hours; the pattern takes the rest.
const RATE_WEIGHT: f64 = 0.5;
/// Snapshot peaks needed before they replace the cost-based peak.
const MIN_PEAKS: usize = 3;
/// Most recent windows behind a typical peak.
const PEAK_WINDOWS: usize = 20;

/// Says when the weekly window runs out, or how much of it is left at its reset.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Forecast {
    /// The window reaches 100 % at `t`.
    RunsOut { t: u64 },
    /// The window resets with `pct` percent unused.
    Unused { pct: f64 },
}

/// Holds the five-hour windows of the day of `now`.
#[derive(Clone, Debug, Serialize)]
pub struct FiveHourDay {
    /// The weekday of `now` in UTC, `Mon` to `Sun`.
    pub weekday: &'static str,
    /// Expected windows that start on this weekday, per day.
    pub expected_windows: Option<f64>,
    /// Windows that started today, in UTC.
    pub started_today: usize,
    /// The typical highest share of a window, in percent.
    pub typical_peak: Option<f64>,
    pub peak_source: Option<PeakSource>,
}

/// Names where the typical peak comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeakSource {
    /// The highest share of each window in the snapshots, over `n` windows.
    Snapshots { n: usize },
    /// The list-price cost of each window times `k`, over `n` windows.
    Cost { n: usize },
}

/// Returns when the weekly window at `used` percent runs out at the expected pace, or how much of
/// it is unused at `span.end`. `k` is percent per USD and `usd_per_hour` the 60-minute rate.
pub(super) fn week(
    used: f64,
    k: f64,
    usd_per_hour: f64,
    now: u64,
    span: Span,
    expect: &Expect,
) -> Forecast {
    if used >= 100.0 {
        return Forecast::RunsOut { t: now };
    }
    let mut pct = used;
    let mut t = now;
    while t < span.end {
        let next = (hour_start(t) + HOUR_S).min(span.end);
        let frac = (next - t) as f64 / HOUR_S as f64;
        let pattern = expect.at(t, Some(span)) * frac;
        let usd = if t < now + RATE_S {
            RATE_WEIGHT * usd_per_hour * frac + (1.0 - RATE_WEIGHT) * pattern
        } else {
            pattern
        };
        let gain = k * usd;
        if pct + gain >= 100.0 {
            let into = (100.0 - pct) / gain;
            return Forecast::RunsOut {
                t: t + (into * (next - t) as f64) as u64,
            };
        }
        pct += gain;
        t = next;
    }
    Forecast::Unused { pct: 100.0 - pct }
}

/// Summarises the five-hour windows of the day of `now`. `k` is the calibration of that window,
/// used for the peak when the snapshots have too few.
pub(super) fn five_hour_day(
    costs: &Costed,
    snapshots: &[Snapshot],
    k: Option<f64>,
    now: u64,
) -> FiveHourDay {
    let starts = five_hour_starts(costs);
    let today = day_start(now);
    let (typical_peak, peak_source) = peak(costs, snapshots, k, &starts, now);
    FiveHourDay {
        weekday: WEEKDAYS[weekday(now)],
        expected_windows: expected_per_day(&starts, now),
        started_today: starts.iter().filter(|&&s| s >= today && s < now).count(),
        typical_peak,
        peak_source,
    }
}

/// Returns the expected number of windows that start on the weekday of `now`, per day, over the
/// complete days of the raw requests, recent days weighing more.
fn expected_per_day(starts: &[u64], now: u64) -> Option<f64> {
    let first = *starts.first()?;
    let today = day_start(now);
    let mut day = day_start(first);
    if day < first {
        day += DAY_S;
    }
    let (mut sum, mut weight_sum) = (0.0, 0.0);
    while day < today {
        if weekday(day) == weekday(now) {
            let n = starts
                .iter()
                .filter(|&&s| (day..day + DAY_S).contains(&s))
                .count() as f64;
            let weight = 0.5f64.powf((now - day) as f64 / HALF_LIFE_S);
            sum += n * weight;
            weight_sum += weight;
        }
        day += DAY_S;
    }
    (weight_sum > 0.0).then(|| sum / weight_sum)
}

/// Returns the typical highest share of a five-hour window and its source: the snapshots when
/// they have enough peaks, else the cost of the completed windows times `k`.
fn peak(
    costs: &Costed,
    snapshots: &[Snapshot],
    k: Option<f64>,
    starts: &[u64],
    now: u64,
) -> (Option<f64>, Option<PeakSource>) {
    let peaks = snapshot_peaks(snapshots);
    if peaks.len() >= MIN_PEAKS {
        let source = PeakSource::Snapshots {
            n: peaks.len().min(PEAK_WINDOWS),
        };
        return (typical(&peaks, PEAK_WINDOWS), Some(source));
    }
    let Some(k) = k else {
        return (None, None);
    };
    let len = length(Window::FiveHour);
    let shares: Vec<f64> = starts
        .iter()
        .filter(|&&s| s + len <= now)
        .map(|&s| k * costs.between(s, s + len))
        .collect();
    match typical(&shares, PEAK_WINDOWS) {
        Some(p) => (
            Some(p),
            Some(PeakSource::Cost {
                n: shares.len().min(PEAK_WINDOWS),
            }),
        ),
        None => (None, None),
    }
}

/// Returns the highest share each five-hour window reached in the snapshots, in reset order.
fn snapshot_peaks(snapshots: &[Snapshot]) -> Vec<f64> {
    let mut by_reset: BTreeMap<u64, f64> = BTreeMap::new();
    for s in snapshots.iter().filter(|s| s.window == Window::FiveHour) {
        let peak = by_reset.entry(s.resets_at).or_insert(0.0);
        *peak = peak.max(s.used_percentage);
    }
    by_reset.into_values().collect()
}
