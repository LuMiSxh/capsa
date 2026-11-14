// SPDX-License-Identifier: MPL-2.0
//! Estimates the subscription limit windows from the list-price cost of the ledger and the limit
//! snapshots, and forecasts the weekly window from the usage pattern. Returns values only: the
//! `census limits` and `census usage --by` commands print them.

mod calibrate;
mod clock;
mod cost;
mod daily;
mod error;
mod forecast;
mod month;
mod pattern;
mod window;
pub use clock::month_start;

#[cfg(test)]
mod tests;

pub use calibrate::Calibration;
pub use clock::when;
pub use daily::{DailySource, DailyStatus};
pub use error::LimitsError;
pub use forecast::{FiveHourDay, Forecast, PeakSource};
pub use month::{MonthStatus, Today, compute_month, month, month_stored};
pub use pattern::{Bucket, Kind, Pattern};

use crate::ledger::{Ledger, Opts, Priced};
use capsa_core::census_snapshot::{self, Snapshot, Window};
use clock::HOUR_S;
use cost::Costed;
use pattern::{Expect, WEEK_HOURS};
use serde::Serialize;
use std::path::Path;
use std::time::Instant;
use window::Span;

/// A snapshot this young is the reading itself; an older one only calibrates the estimate.
const FRESH_S: u64 = 120;
/// Length of the short burn window, in seconds.
const QUARTER_HOUR_S: u64 = 900;
/// The short burn window holds at least this much list-price cost before its rate is compared:
/// one request alone would say more than the hour's habit.
const SURGE_MIN_USD: f64 = 0.25;

/// The ledger and snapshot data that the limit state is computed from.
pub struct History<'a> {
    pub priced: &'a Priced,
    pub snapshots: &'a [Snapshot],
}

/// Holds the state of the subscription limits at one time.
#[derive(Clone, Debug, Serialize)]
pub struct LimitStatus {
    pub now: u64,
    /// False when the ledger and the snapshots hold nothing.
    pub data: bool,
    /// The five-hour window, then the weekly one.
    pub windows: Vec<WindowStatus>,
    pub burn: Burn,
    /// The forecast of the weekly window; `None` without a weekly calibration.
    pub forecast: Option<Forecast>,
    /// The five-hour burn against the typical rate of this hour; `None` when it is not comparable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surge: Option<Surge>,
    pub five_hour: FiveHourDay,
    pub basis: Basis,
}

/// Holds the running window of one limit and what is known about its share.
#[derive(Clone, Debug, Serialize)]
pub struct WindowStatus {
    pub window: Window,
    /// First second of the running window.
    pub start: Option<u64>,
    /// Reset of the running window.
    pub reset_at: Option<u64>,
    /// Share of the window used, in percent.
    pub used: Option<f64>,
    pub reading: Option<Reading>,
    pub calibration: Option<Calibration>,
    pub full: Option<Full>,
}

/// Names where the share of a window comes from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reading {
    /// The newest snapshot, `age_s` seconds old, is the share.
    Snapshot { age_s: u64 },
    /// The share is the calibration times the cost since the window start. `snapshot_age_s` is the
    /// age of the newest snapshot, if there is one.
    Estimated { snapshot_age_s: Option<u64> },
}

/// Says when a window is full at the current rate.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Full {
    /// The window is full at `t`.
    At { t: u64 },
    /// The window lasts until its reset.
    AfterReset,
}

/// The list-price cost per hour over the last 15 minutes and the last 60 minutes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Burn {
    pub usd_per_hour_15: f64,
    pub usd_per_hour_60: f64,
}

/// Holds the burn of the last 15 minutes against the rate this hour usually costs.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Surge {
    /// Expected USD of this hour from the history: the typical rate, above zero.
    pub typical_usd_per_hour: f64,
    /// The five-hour window at the 15-minute rate: full at this time, or lasting to its reset.
    pub full: Full,
}

/// Describes the history that the forecast and the tables were expected from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Basis {
    /// Hours of history, from its first record to the hour before now.
    pub hours: usize,
    /// `hours` in weeks.
    pub weeks: f64,
    /// Weeks of those hours that fall in a weekly window the snapshots name.
    pub mapped_weeks: f64,
    pub pattern: Pattern,
}

/// The buckets of `census usage --by`, with the basis they were expected from.
#[derive(Clone, Debug, Serialize)]
pub struct Table {
    pub kind: Kind,
    pub basis: Basis,
    pub rows: Vec<Bucket>,
}

impl Table {
    /// Returns whether the history holds any hour.
    pub fn has_data(&self) -> bool {
        self.basis.hours > 0
    }
}

/// Returns the limit state at `now` from the ledger and the snapshot file.
///
/// The ledger is updated first, which costs one incremental read of the transcripts.
///
/// # Errors
/// Fails when the ledger or the snapshot file cannot be read or written.
pub fn status(now: u64) -> Result<LimitStatus, LimitsError> {
    status_by(now, None, None)
}

/// Like [`status`], but the ledger update stops reading transcripts at `deadline`, keeps its
/// progress and the limits are computed from what is read so far. `current` is the running
/// session's transcript, which a fresh sweep reads alone (see [`crate::ledger::Opts::current`]).
///
/// # Errors
/// Fails when the ledger or the snapshot file cannot be read or written.
pub fn status_by(
    now: u64,
    deadline: Option<Instant>,
    current: Option<&Path>,
) -> Result<LimitStatus, LimitsError> {
    let (ledger, snapshots) = load(now, deadline, current)?;
    let priced = ledger.priced()?;
    let history = History {
        priced: &priced,
        snapshots: &snapshots,
    };
    Ok(compute(&history, now))
}

/// Returns the expected cost of each bucket of `kind` from the ledger and the snapshot file.
///
/// # Errors
/// Fails when the ledger or the snapshot file cannot be read or written.
pub fn usage_by(now: u64, kind: Kind) -> Result<Table, LimitsError> {
    let (ledger, snapshots) = load(now, None, None)?;
    let costs = Costed::from_priced(&ledger.priced()?);
    let expect = Expect::build(&costs, weekly_spans(&snapshots), now);
    Ok(Table {
        kind,
        basis: basis(&expect),
        rows: expect.table(kind),
    })
}

fn load(
    now: u64,
    deadline: Option<Instant>,
    current: Option<&Path>,
) -> Result<(Ledger, Vec<Snapshot>), LimitsError> {
    let ledger = Ledger::open_and_update(&Opts {
        deadline,
        current: current.map(Path::to_path_buf),
        ..Opts::defaults(now)
    })?;
    let snapshots = census_snapshot::read_since(0)?;
    Ok((ledger, snapshots))
}

/// Returns the limit state at `now` computed from `history`. Reads nothing, so tests can feed it
/// rows and snapshots.
pub fn compute(history: &History, now: u64) -> LimitStatus {
    let costs = Costed::from_priced(history.priced);
    let burn = Burn {
        usd_per_hour_15: costs.between(now.saturating_sub(QUARTER_HOUR_S), now) * 4.0,
        usd_per_hour_60: costs.between(now.saturating_sub(HOUR_S), now),
    };
    let five = estimate(
        Window::FiveHour,
        history.snapshots,
        &costs,
        now,
        burn.usd_per_hour_60,
    );
    let seven = estimate(
        Window::SevenDay,
        history.snapshots,
        &costs,
        now,
        burn.usd_per_hour_60,
    );
    let expect = Expect::build(&costs, weekly_spans(history.snapshots), now);
    let surge = surge(&five, burn, &expect, seven.span, now);
    let forecast = match (seven.used, seven.status.calibration, seven.span) {
        (Some(used), Some(c), Some(span)) => Some(forecast::week(
            used,
            c.k,
            burn.usd_per_hour_60,
            now,
            span,
            &expect,
        )),
        _ => None,
    };
    let five_hour = forecast::five_hour_day(
        &costs,
        history.snapshots,
        five.status.calibration.map(|c| c.k),
        now,
    );
    LimitStatus {
        now,
        data: !(history.priced.is_empty() && history.snapshots.is_empty()),
        windows: vec![five.status, seven.status],
        burn,
        forecast,
        surge,
        five_hour,
        basis: basis(&expect),
    }
}

/// Returns the surge of the five-hour window: its 15-minute burn is comparable when its share is
/// known and calibrated, the typical rate of this hour is above zero, and the 15 minutes hold at
/// least `SURGE_MIN_USD`.
fn surge(
    five: &Estimate,
    burn: Burn,
    expect: &Expect,
    week: Option<Span>,
    now: u64,
) -> Option<Surge> {
    let (used, k, span) = (five.used?, five.status.calibration?.k, five.span?);
    let typical_usd_per_hour = expect.at(now, week);
    let cost_15 = burn.usd_per_hour_15 * QUARTER_HOUR_S as f64 / HOUR_S as f64;
    if typical_usd_per_hour <= 0.0 || cost_15 < SURGE_MIN_USD {
        return None;
    }
    Some(Surge {
        typical_usd_per_hour,
        full: full_at(used, k, burn.usd_per_hour_15, now, span.end),
    })
}

/// One window's running span and share, with the status that reports them.
struct Estimate {
    span: Option<Span>,
    used: Option<f64>,
    status: WindowStatus,
}

fn estimate(
    window: Window,
    snapshots: &[Snapshot],
    costs: &Costed,
    now: u64,
    usd_per_hour: f64,
) -> Estimate {
    let own: Vec<Snapshot> = snapshots
        .iter()
        .filter(|s| s.window == window)
        .copied()
        .collect();
    let calibration = calibrate::calibrate(window, &own, costs, now);
    let latest = own.iter().max_by_key(|s| s.t).copied();
    let span = latest.and_then(|s| window::current(window, &s, costs, now));
    let (used, reading) = match (latest, span, calibration) {
        (Some(s), _, _) if now.saturating_sub(s.t) < FRESH_S => (
            Some(s.used_percentage),
            Some(Reading::Snapshot {
                age_s: now.saturating_sub(s.t),
            }),
        ),
        (_, Some(span), Some(c)) => (
            Some(c.k * costs.between(span.start, now)),
            Some(Reading::Estimated {
                snapshot_age_s: latest.map(|s| now.saturating_sub(s.t)),
            }),
        ),
        _ => (None, None),
    };
    let full = match (used, calibration, span) {
        (Some(used), Some(c), Some(span)) => Some(full_at(used, c.k, usd_per_hour, now, span.end)),
        _ => None,
    };
    Estimate {
        span,
        used,
        status: WindowStatus {
            window,
            start: span.map(|s| s.start),
            reset_at: span.map(|s| s.end),
            used,
            reading,
            calibration,
            full,
        },
    }
}

/// Returns when a window at `used` percent reaches 100 % at the current rate, or that it lasts
/// until `end`.
fn full_at(used: f64, k: f64, usd_per_hour: f64, now: u64, end: u64) -> Full {
    if used >= 100.0 {
        return Full::At { t: now };
    }
    let pct_per_s = k * usd_per_hour / HOUR_S as f64;
    if pct_per_s <= 0.0 {
        return Full::AfterReset;
    }
    let t = now.saturating_add(((100.0 - used) / pct_per_s) as u64);
    if t < end {
        Full::At { t }
    } else {
        Full::AfterReset
    }
}

/// Returns the weekly windows that the snapshots name, oldest first.
fn weekly_spans(snapshots: &[Snapshot]) -> Vec<Span> {
    let mut resets: Vec<u64> = snapshots
        .iter()
        .filter(|s| s.window == Window::SevenDay)
        .map(|s| s.resets_at)
        .collect();
    resets.sort_unstable();
    resets.dedup();
    let len = window::length(Window::SevenDay);
    resets
        .into_iter()
        .map(|end| Span {
            start: end.saturating_sub(len),
            end,
        })
        .collect()
}

fn basis(expect: &Expect) -> Basis {
    let week = WEEK_HOURS as f64;
    Basis {
        hours: expect.hours,
        weeks: expect.hours as f64 / week,
        mapped_weeks: expect.mapped_hours as f64 / week,
        pattern: expect.pattern(),
    }
}
