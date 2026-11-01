// SPDX-License-Identifier: MPL-2.0
//! Expects the list-price cost of each hour from the history, weighting recent weeks more. An hour
//! is keyed by its weekday and hour of day and, once the snapshots map two weeks of hours to their
//! weekly windows, by its hours since the reset of that window.

use super::clock::{DAY_S, HOUR_S, WEEKDAYS, hour_of_day, hour_start, weekday};
use super::cost::Costed;
use super::window::Span;
use serde::Serialize;

/// Hours in a week: the buckets of the weekday and hour key and of the since-reset key.
pub(super) const WEEK_HOURS: usize = 168;
/// The weight of an hour halves for each of these seconds of its age: four weeks.
pub(super) const HALF_LIFE_S: f64 = 28.0 * DAY_S as f64;
/// Weeks of mapped hours before the since-reset key replaces the weekday and hour key.
const CYCLE_WEEKS: usize = 2;

/// Names what `census usage --by` groups the expected cost by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The expected cost of each day of the week.
    Weekday,
    /// The expected cost of each hour of the day, averaged over the weekdays.
    Hour,
    /// The expected cost of each hour since the weekly reset.
    Cycle,
}

/// Names the key the expected cost was taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    SinceReset,
    WeekdayHour,
    NoHistory,
}

impl Pattern {
    /// Returns the key in words.
    pub fn label(self) -> &'static str {
        match self {
            Pattern::SinceReset => "since the weekly reset",
            Pattern::WeekdayHour => "weekday and hour",
            Pattern::NoHistory => "no history",
        }
    }
}

/// One row of a table: its expected USD, its share of the table and the hours behind it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bucket {
    pub index: usize,
    pub label: String,
    pub usd: f64,
    pub share: f64,
    pub hours: usize,
}

/// The expected USD of an hour for one key, and the hours of history behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Mean {
    pub usd: f64,
    pub hours: usize,
}

impl Mean {
    fn join(self, o: Mean) -> Mean {
        Mean {
            usd: self.usd + o.usd,
            hours: self.hours + o.hours,
        }
    }
}

/// Sums the weighted USD of one key while the history is walked.
#[derive(Clone, Copy, Default)]
struct Acc {
    weighted: f64,
    weight: f64,
    hours: usize,
}

impl Acc {
    fn add(&mut self, usd: f64, weight: f64) {
        self.weighted += usd * weight;
        self.weight += weight;
        self.hours += 1;
    }

    fn mean(self) -> Mean {
        let usd = if self.weight > 0.0 {
            self.weighted / self.weight
        } else {
            0.0
        };
        Mean {
            usd,
            hours: self.hours,
        }
    }
}

/// Holds the expected USD of an hour for each key, built from the history up to `now`.
pub(super) struct Expect {
    /// Hours of history, from its first record to the hour before `now`.
    pub hours: usize,
    /// Hours of that history that fall in a weekly window of the snapshots.
    pub mapped_hours: usize,
    weekday_hour: Vec<Mean>,
    cycle: Vec<Mean>,
    cycle_on: bool,
    windows: Vec<Span>,
}

impl Expect {
    /// Builds the expectations from the hours of `costs` up to the hour of `now`. `windows` are the
    /// weekly windows the snapshots name.
    pub(super) fn build(costs: &Costed, mut windows: Vec<Span>, now: u64) -> Expect {
        windows.sort_by_key(|s| s.start);
        let mut weekday_hour = vec![Acc::default(); WEEK_HOURS];
        let mut cycle = vec![Acc::default(); WEEK_HOURS];
        let mut hours = 0;
        let mut mapped_hours = 0;
        if let Some(first) = costs.first_hour() {
            let series = costs.hour_costs();
            let mut t = first;
            while t < hour_start(now) {
                let usd = series.get(&t).copied().unwrap_or(0.0);
                let weight = 0.5f64.powf((now - t) as f64 / HALF_LIFE_S);
                weekday_hour[weekday_hour_index(t)].add(usd, weight);
                if let Some(i) = cycle_index(&windows, t) {
                    cycle[i].add(usd, weight);
                    mapped_hours += 1;
                }
                hours += 1;
                t += HOUR_S;
            }
        }
        Expect {
            hours,
            mapped_hours,
            weekday_hour: weekday_hour.into_iter().map(Acc::mean).collect(),
            cycle: cycle.into_iter().map(Acc::mean).collect(),
            cycle_on: mapped_hours >= CYCLE_WEEKS * WEEK_HOURS,
            windows,
        }
    }

    /// Returns the key the expectations come from.
    pub(super) fn pattern(&self) -> Pattern {
        if self.hours == 0 {
            Pattern::NoHistory
        } else if self.cycle_on {
            Pattern::SinceReset
        } else {
            Pattern::WeekdayHour
        }
    }

    /// Returns the expected USD of the hour containing `t`. The since-reset key is used when it is
    /// on and has history there. `current` is the running weekly window, which the history may not
    /// name yet.
    pub(super) fn at(&self, t: u64, current: Option<Span>) -> f64 {
        let weekday_hour = self.weekday_hour[weekday_hour_index(t)].usd;
        if !self.cycle_on {
            return weekday_hour;
        }
        let index =
            cycle_index(&self.windows, t).or_else(|| current.and_then(|s| cycle_index(&[s], t)));
        match index.map(|i| self.cycle[i]) {
            Some(m) if m.hours > 0 => m.usd,
            _ => weekday_hour,
        }
    }

    /// Returns the buckets of `kind`, each with its share of their total.
    pub(super) fn table(&self, kind: Kind) -> Vec<Bucket> {
        let cell = |d: usize, h: usize| self.weekday_hour[d * 24 + h];
        let rows: Vec<(String, Mean)> = match kind {
            Kind::Weekday => (0..7)
                .map(|d| {
                    let day = (0..24)
                        .map(|h| cell(d, h))
                        .fold(Mean::default(), Mean::join);
                    (WEEKDAYS[d].to_owned(), day)
                })
                .collect(),
            Kind::Hour => (0..24)
                .map(|h| {
                    let mut mean = (0..7).map(|d| cell(d, h)).fold(Mean::default(), Mean::join);
                    mean.usd /= 7.0;
                    (format!("{h:02}:00"), mean)
                })
                .collect(),
            Kind::Cycle => (0..WEEK_HOURS)
                .map(|i| {
                    let mean = if self.cycle_on {
                        self.cycle[i]
                    } else {
                        Mean::default()
                    };
                    (format!("+{i}h"), mean)
                })
                .collect(),
        };
        let total: f64 = rows.iter().map(|(_, m)| m.usd).sum();
        rows.into_iter()
            .enumerate()
            .map(|(index, (label, m))| Bucket {
                index,
                label,
                usd: m.usd,
                share: if total > 0.0 { m.usd / total } else { 0.0 },
                hours: m.hours,
            })
            .collect()
    }
}

/// Returns the hour since the reset of the weekly window that covers `t`, from 0 to 167. Where two
/// windows overlap, as after an early reset, the one that started last covers `t`.
pub(super) fn cycle_index(windows: &[Span], t: u64) -> Option<usize> {
    let i = windows.partition_point(|s| s.start <= t).checked_sub(1)?;
    let w = windows.get(i)?;
    (t < w.end)
        .then(|| ((t - w.start) / HOUR_S) as usize)
        .filter(|h| *h < WEEK_HOURS)
}

/// Returns the index of the local weekday and hour of `t`, Monday 00:00 as 0.
fn weekday_hour_index(t: u64) -> usize {
    weekday(t) * 24 + hour_of_day(t)
}
