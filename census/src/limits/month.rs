// SPDX-License-Identifier: MPL-2.0
//! Sums the list-price spend of the current local calendar month and projects it to the end of the
//! month, for the budget mode.

use super::clock::{DAY_S, day_start, days_until, month_start, next_month_start};
use super::cost::Costed;
use super::{History, LimitsError};
use crate::ledger::{Ledger, Opts};
use serde::Serialize;
use std::path::Path;
use std::time::Instant;

/// The part of the month that is the local day of `now`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Today {
    /// First second of the local day, its midnight.
    pub start: u64,
    /// The spend since `start`.
    pub usd: f64,
    /// Local days left in the month, the day of `now` included.
    pub days_left: u64,
}

/// The spend of one local calendar month against an optional budget.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct MonthStatus {
    pub now: u64,
    /// False when the ledger holds nothing.
    pub data: bool,
    /// First second of the month, its local midnight on the 1st.
    pub start: u64,
    /// First second of the next month.
    pub end: u64,
    pub elapsed_s: u64,
    pub spent_usd: f64,
    pub budget_usd: Option<f64>,
    /// Share of the budget spent, in percent.
    pub percent: Option<f64>,
    /// Days until the month ends, counting a started day as whole.
    pub days_left: u64,
    /// The spend at the end of the month if the month-to-date daily rate continues.
    pub projected_usd: f64,
    /// Whether the projection exceeds the budget.
    pub projected_over: bool,
    /// What is left of the budget for each remaining day; 0 once the budget is spent.
    pub per_day_left_usd: Option<f64>,
    /// The spend of the local day of `now`; read by the daily limit, not printed with the month.
    #[serde(skip)]
    pub today: Today,
}

impl MonthStatus {
    /// Builds the status of the month from `start` to `end` at `now` for a spend of `spent_usd`.
    ///
    /// The rate is the spend over the days elapsed, at least one day, so that the first hours of a
    /// month do not project a single request across the whole month.
    pub fn new(
        spent_usd: f64,
        start: u64,
        end: u64,
        now: u64,
        budget_usd: Option<f64>,
        data: bool,
        today: Today,
    ) -> Self {
        // An empty sum is -0.0, which prints as `-0`; adding 0.0 turns it into 0.0.
        let spent_usd = spent_usd + 0.0;
        let elapsed_s = now.saturating_sub(start);
        let length_s = end.saturating_sub(start);
        let projected_usd =
            (spent_usd * length_s as f64 / elapsed_s.max(DAY_S) as f64).max(spent_usd);
        let days_left = end.saturating_sub(now).div_ceil(DAY_S);
        let budget = budget_usd.filter(|b| *b > 0.0);
        MonthStatus {
            now,
            data,
            start,
            end,
            elapsed_s,
            spent_usd,
            budget_usd: budget,
            percent: budget.map(|b| 100.0 * spent_usd / b),
            days_left,
            projected_usd,
            projected_over: budget.is_some_and(|b| projected_usd > b),
            per_day_left_usd: budget
                .filter(|_| days_left > 0)
                .map(|b| (b - spent_usd).max(0.0) / days_left as f64),
            today,
        }
    }
}

/// Returns the month status at `now` computed from `history`. Reads nothing.
pub fn compute_month(history: &History, now: u64, budget_usd: Option<f64>) -> MonthStatus {
    let costs = Costed::from_priced(history.priced);
    let (start, end) = (month_start(now), next_month_start(now));
    let data = !history.priced.is_empty();
    let day = day_start(now);
    let today = Today {
        start: day,
        usd: costs.between(day, now),
        days_left: days_until(day, end),
    };
    MonthStatus::new(
        costs.between(start, now),
        start,
        end,
        now,
        budget_usd,
        data,
        today,
    )
}

/// Returns the month status at `now` from the ledger as it is, without reading transcripts, for
/// the status line.
///
/// # Errors
/// Fails when the ledger files cannot be read.
pub fn month_stored(now: u64, budget_usd: Option<f64>) -> Result<MonthStatus, LimitsError> {
    from_ledger(
        &Ledger::open(&capsa_core::paths::state_dir("census")),
        now,
        budget_usd,
    )
}

/// Returns the month status at `now` after a ledger update that stops reading at `deadline`.
/// `current` is the running session's transcript, which a fresh sweep reads alone.
///
/// # Errors
/// Fails when the ledger cannot be read or written.
pub fn month(
    now: u64,
    budget_usd: Option<f64>,
    deadline: Option<Instant>,
    current: Option<&Path>,
) -> Result<MonthStatus, LimitsError> {
    let ledger = Ledger::open_and_update(&Opts {
        deadline,
        current: current.map(Path::to_path_buf),
        ..Opts::defaults(now)
    })?;
    from_ledger(&ledger, now, budget_usd)
}

fn from_ledger(
    ledger: &Ledger,
    now: u64,
    budget_usd: Option<f64>,
) -> Result<MonthStatus, LimitsError> {
    let priced = ledger.priced()?;
    let history = History {
        priced: &priced,
        snapshots: &[],
    };
    Ok(compute_month(&history, now, budget_usd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Hourly, Priced, Tokens};

    const START: u64 = 1_000_000;

    fn status(spent: f64, elapsed_s: u64, budget: Option<f64>) -> MonthStatus {
        MonthStatus::new(
            spent,
            START,
            START + 30 * DAY_S,
            START + elapsed_s,
            budget,
            true,
            Today::default(),
        )
    }

    #[test]
    fn projection_is_the_month_to_date_rate_over_the_month() {
        let s = status(50.0, 10 * DAY_S, Some(200.0));
        assert!((s.projected_usd - 150.0).abs() < 1e-9);
        assert!(!s.projected_over);
        assert!((s.percent.unwrap() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn a_projection_above_the_budget_is_flagged() {
        let s = status(100.0, 10 * DAY_S, Some(200.0));
        assert!((s.projected_usd - 300.0).abs() < 1e-9);
        assert!(s.projected_over);
    }

    #[test]
    fn remaining_budget_is_spread_over_the_days_left() {
        let s = status(50.0, 10 * DAY_S, Some(200.0));
        assert_eq!(s.days_left, 20);
        assert!((s.per_day_left_usd.unwrap() - 7.5).abs() < 1e-9);
    }

    #[test]
    fn a_spent_budget_leaves_nothing_per_day() {
        let s = status(250.0, 10 * DAY_S, Some(200.0));
        assert_eq!(s.per_day_left_usd, Some(0.0));
        assert!(s.percent.unwrap() > 100.0);
    }

    #[test]
    fn the_first_hours_do_not_scale_up_a_single_request() {
        let s = status(2.0, 3600, Some(200.0));
        assert!((s.projected_usd - 60.0).abs() < 1e-9);
    }

    #[test]
    fn without_a_budget_there_is_no_share_or_flag() {
        let s = status(50.0, 10 * DAY_S, None);
        assert_eq!((s.percent, s.per_day_left_usd), (None, None));
        assert!(!s.projected_over);
    }

    #[test]
    fn the_last_started_day_counts_as_a_day_left() {
        let s = status(1.0, 30 * DAY_S - 60, None);
        assert_eq!(s.days_left, 1);
    }

    #[test]
    fn only_spend_from_the_month_start_counts() {
        let now = 1_800_000_000;
        let start = month_start(now);
        let hour = |hour| Hourly {
            hour,
            family: "opus".to_owned(),
            sub: false,
            tokens: Tokens {
                output: 1_000_000,
                ..Tokens::default()
            },
        };
        let hourly = [hour(start - 3600), hour(start), hour(now - 3600)];
        let priced = Priced::of(&[], &hourly);
        let history = History {
            priced: &priced,
            snapshots: &[],
        };
        let m = compute_month(&history, now, Some(1000.0));
        let one = Costed::new(&[], &hourly[1..2]).between(0, u64::MAX - 1);
        assert!(one > 0.0);
        assert!((m.spent_usd - 2.0 * one).abs() < 1e-9);
        assert!(m.data);
    }

    #[test]
    fn today_is_the_spend_since_local_midnight_and_counts_its_own_day_left() {
        let now = 1_800_000_000;
        let today = day_start(now);
        let hour = |hour| Hourly {
            hour,
            family: "opus".to_owned(),
            sub: false,
            tokens: Tokens {
                output: 1_000_000,
                ..Tokens::default()
            },
        };
        let hourly = [hour(today), hour(today - DAY_S)];
        let priced = Priced::of(&[], &hourly);
        let history = History {
            priced: &priced,
            snapshots: &[],
        };
        let m = compute_month(&history, now, Some(1000.0));
        let one = Costed::new(&[], &hourly[..1]).between(0, u64::MAX - 1);
        assert!((m.today.usd - one).abs() < 1e-9);
        assert!((m.spent_usd - 2.0 * one).abs() < 1e-9);
        assert_eq!(m.today.start, today);
        assert_eq!(m.today.days_left, (m.end - today) / DAY_S);
    }
}
