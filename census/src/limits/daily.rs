// SPDX-License-Identifier: MPL-2.0
//! Sizes the daily limit of the budget mode and tells how far the spend of today has reached it.

use super::month::MonthStatus;
use serde::Serialize;

/// Where a daily limit comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DailySource {
    /// What is left of the monthly budget, shared over the local days left.
    Derived,
    /// The limit set with `census self config set daily-limit <usd>`.
    Fixed,
}

impl DailySource {
    /// Returns the name of the source as it is printed and written in JSON.
    pub fn name(self) -> &'static str {
        match self {
            DailySource::Derived => "derived",
            DailySource::Fixed => "fixed",
        }
    }
}

/// The spend of today against its daily limit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DailyStatus {
    /// The spend since the local midnight of today, in USD.
    pub usd: f64,
    /// The limit of today in USD; 0 when the monthly budget is already spent.
    pub limit: f64,
    pub source: DailySource,
    /// The share of the limit spent today. With a limit of 0 it is 100 once anything is spent.
    pub percent: f64,
}

impl DailyStatus {
    /// Returns the spend of today against its limit, with the share and the source, for a line.
    pub fn text(&self) -> String {
        format!(
            "${:.2} of ${:.2} ({:.0} %, {})",
            self.usd,
            self.limit,
            self.percent,
            self.source.name()
        )
    }

    /// Returns today's status of `month`, with `fixed_usd` as the limit when it is set and the
    /// share of the monthly budget that is left otherwise. `None` when neither is set.
    pub fn of(month: &MonthStatus, fixed_usd: Option<f64>) -> Option<DailyStatus> {
        let (limit, source) = match (fixed_usd, month.budget_usd) {
            (Some(usd), _) => (usd, DailySource::Fixed),
            (None, Some(budget)) => (derived(budget, month), DailySource::Derived),
            (None, None) => return None,
        };
        let usd = month.today.usd;
        let percent = if limit > 0.0 {
            100.0 * usd / limit
        } else if usd > 0.0 {
            100.0
        } else {
            0.0
        };
        Some(DailyStatus {
            usd,
            limit,
            source,
            percent,
        })
    }
}

/// Shares what is left of the budget before today over the local days left, today included.
fn derived(budget: f64, month: &MonthStatus) -> f64 {
    let before_today = (month.spent_usd - month.today.usd).max(0.0);
    (budget - before_today).max(0.0) / month.today.days_left.max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::month::Today;

    const START: u64 = 1_000_000;
    const DAY: u64 = 86_400;

    /// A 30-day month whose today has `today_usd` spent of `spent` in all, with `days_left`.
    fn month(spent: f64, today_usd: f64, days_left: u64, budget: Option<f64>) -> MonthStatus {
        let today = Today {
            start: START + 20 * DAY,
            usd: today_usd,
            days_left,
        };
        MonthStatus::new(
            spent,
            START,
            START + 30 * DAY,
            START + 20 * DAY + 3600,
            budget,
            true,
            today,
        )
    }

    #[test]
    fn the_derived_limit_shares_the_budget_left_before_today_over_the_days_left() {
        let d = DailyStatus::of(&month(50.0, 2.0, 10, Some(100.0)), None).unwrap();
        assert_eq!(d.source, DailySource::Derived);
        assert!((d.limit - 5.2).abs() < 1e-9);
        assert!((d.percent - 100.0 * 2.0 / 5.2).abs() < 1e-9);
    }

    #[test]
    fn the_last_day_of_the_month_gets_all_that_is_left() {
        let d = DailyStatus::of(&month(90.0, 0.0, 1, Some(100.0)), None).unwrap();
        assert!((d.limit - 10.0).abs() < 1e-9);
        assert_eq!(d.percent, 0.0);
    }

    #[test]
    fn an_overspent_month_floors_the_limit_at_zero_so_any_spend_is_full() {
        let spent = month(250.0, 0.5, 10, Some(200.0));
        let d = DailyStatus::of(&spent, None).unwrap();
        assert_eq!(d.limit, 0.0);
        assert_eq!(d.percent, 100.0);
        let quiet = DailyStatus::of(&month(250.0, 0.0, 10, Some(200.0)), None).unwrap();
        assert_eq!(quiet.percent, 0.0);
    }

    #[test]
    fn a_fixed_limit_replaces_the_derived_one_and_needs_no_budget() {
        let d = DailyStatus::of(&month(50.0, 4.0, 10, Some(100.0)), Some(8.0)).unwrap();
        assert_eq!((d.source, d.limit), (DailySource::Fixed, 8.0));
        assert_eq!(d.percent, 50.0);
        let no_budget = DailyStatus::of(&month(50.0, 4.0, 10, None), Some(8.0)).unwrap();
        assert_eq!(no_budget.source, DailySource::Fixed);
    }

    #[test]
    fn without_a_budget_or_a_fixed_limit_there_is_no_daily_limit() {
        assert_eq!(DailyStatus::of(&month(50.0, 4.0, 10, None), None), None);
    }
}
