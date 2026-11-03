// SPDX-License-Identifier: MPL-2.0
//! `census limits`: the subscription limit windows as they stand, the weekly forecast and the
//! usage pattern behind it, or the spend of the month in budget mode.

use crate::now_secs;
use crate::out::Out;
use anyhow::Result;
use capsa_core::census_snapshot::Window;
use capsa_core::exit::Outcome;
use capsa_core::render::{Doc, Render, emit};
use census::config::{Config, Mode};
use census::limits::{
    self, DailyStatus, FiveHourDay, Forecast, Full, LimitStatus, MonthStatus, PeakSource, Reading,
    WindowStatus, when,
};
use census::usage::refresh_prices;
use serde::Serialize;

/// Prints the limit state, or the month summary in budget mode. Exit 1 when the ledger and the
/// snapshot file hold nothing.
pub fn run(out: &Out) -> Result<Outcome> {
    let config = Config::load_from(&Config::path())?;
    refresh_prices(now_secs(), config.prices_refresh_days);
    if config.mode == Mode::Budget {
        return run_budget(&config, out);
    }
    let status = limits::status(now_secs())?;
    emit(&PlanView(&status), out.format)
}

/// The plan view: the windows, the forecast and the pattern of a `LimitStatus`, as its JSON.
#[derive(Serialize)]
#[serde(transparent)]
struct PlanView<'a>(&'a LimitStatus);

impl Render for PlanView<'_> {
    fn doc(&self, doc: &mut Doc) {
        let s = self.0;
        if !s.data {
            doc.line("no usage data yet: no transcript and no limit snapshot to read");
            return;
        }
        doc.title("census limits");
        for w in &s.windows {
            window_row(doc, w);
        }
        for w in &s.windows {
            if let Some(detail) = window_detail(w) {
                doc.line(format!("{} window: {detail}", label(w.window)));
            }
        }
        doc.row("burn").text(format!(
            "{}/h over 15 min, {}/h over 60 min",
            usd(s.burn.usd_per_hour_15),
            usd(s.burn.usd_per_hour_60)
        ));
        doc.row("weekly").text(forecast_text(s.forecast.as_ref()));
        doc.row("5h windows").text(five_hour_text(&s.five_hour));
        doc.row("basis").dim(basis_text(s));
    }

    fn outcome(&self) -> Outcome {
        outcome(self.0.data)
    }
}

/// The budget view: the month against its budget, today against the daily limit, and the rest of
/// the month, as its JSON.
#[derive(Serialize)]
struct BudgetOut<'a> {
    mode: &'static str,
    #[serde(flatten)]
    month: &'a MonthStatus,
    today: Option<DailyStatus>,
}

impl Render for BudgetOut<'_> {
    fn doc(&self, doc: &mut Doc) {
        let m = self.month;
        if !m.data {
            doc.line("no usage data yet: no transcript to read");
            return;
        }
        doc.title("census limits");
        doc.row("mode").text("budget");
        match (m.budget_usd, m.percent) {
            (Some(budget), Some(percent)) => {
                let verdict = if m.projected_over {
                    format!(
                        "forecast {} at the month-to-date rate, {} over the budget",
                        usd(m.projected_usd),
                        usd(m.projected_usd - budget)
                    )
                } else {
                    format!(
                        "forecast {} at the month-to-date rate, within the budget",
                        usd(m.projected_usd)
                    )
                };
                doc.row("month")
                    .bar(percent / 100.0)
                    .text(format!("{} of {}", usd(m.spent_usd), usd(budget)))
                    .text(format!("{percent:.0}%"))
                    .dim(verdict);
            }
            _ => {
                doc.row("month").text(format!(
                    "{}; no budget set: census self config set monthly-budget <usd>",
                    usd(m.spent_usd)
                ));
            }
        }
        if let Some(d) = self.today.as_ref() {
            today_row(doc, d);
        }
        // A blank line starts a new alignment group, so the long texts below do not widen the bars.
        doc.line("");
        if let Some(per_day) = m.per_day_left_usd {
            doc.row("left")
                .text(format!("{} a day for {} days", usd(per_day), m.days_left));
        }
        doc.row("month ends")
            .text(format!("{} days, at {}", m.days_left, when(m.end)));
    }

    fn outcome(&self) -> Outcome {
        outcome(self.month.data)
    }
}

fn run_budget(config: &Config, out: &Out) -> Result<Outcome> {
    let month = limits::month(now_secs(), config.monthly_budget_usd, None)?;
    let body = BudgetOut {
        mode: "budget",
        month: &month,
        today: DailyStatus::of(&month, config.daily_limit_usd),
    };
    emit(&body, out.format)
}

fn outcome(data: bool) -> Outcome {
    if data { Outcome::Found } else { Outcome::Empty }
}

/// A dollar amount. Adding zero turns the `-0.0` of an empty window into `0.0`, which would
/// otherwise print as `$-0.00`.
pub(crate) fn usd(v: f64) -> String {
    format!("${:.2}", v + 0.0)
}

/// Adds the row of today's spend against its daily limit.
pub(crate) fn today_row(doc: &mut Doc, d: &DailyStatus) {
    doc.row("today")
        .bar(d.percent / 100.0)
        .text(format!("{} of {}", usd(d.usd), usd(d.limit)))
        .text(format!("{:.0}%", d.percent))
        .dim(format!("{} limit", d.source.name()));
}

fn label(window: Window) -> &'static str {
    match window {
        Window::FiveHour => "5h",
        Window::SevenDay => "7d",
    }
}

fn snapshot_age(age_s: u64) -> String {
    match age_s {
        s if s < 60 => "snapshot just now".to_owned(),
        s if s < 3600 => format!("snapshot {} min old", s / 60),
        s => format!("snapshot {} h old", s / 3600),
    }
}

fn reading_text(reading: Reading) -> String {
    match reading {
        Reading::Snapshot { age_s } => snapshot_age(age_s),
        Reading::Estimated {
            snapshot_age_s: Some(age),
        } => format!("estimated, {}", snapshot_age(age)),
        Reading::Estimated {
            snapshot_age_s: None,
        } => "estimated".to_owned(),
    }
}

/// Adds the row of a window: its share and reset, or why there is no share.
fn window_row(doc: &mut Doc, w: &WindowStatus) {
    let name = format!("{} window", label(w.window));
    match (w.used, w.reading) {
        (Some(used), Some(_)) => {
            let reset = w
                .reset_at
                .map(|t| format!("resets {}", when(t)))
                .unwrap_or_default();
            doc.row(name)
                .bar(used / 100.0)
                .text(format!("{used:.0}%"))
                .dim(reset);
        }
        _ if w.calibration.is_none() => {
            doc.row(name).dim("no calibration yet: needs a snapshot");
        }
        _ => {
            doc.row(name).dim("no window running");
        }
    }
}

/// Returns the source of a window's share, its rate and its calibration, when there are any.
fn window_detail(w: &WindowStatus) -> Option<String> {
    let mut parts = Vec::new();
    if let (Some(_), Some(reading)) = (w.used, w.reading) {
        parts.push(reading_text(reading));
    }
    match w.full {
        Some(Full::At { t }) => parts.push(format!("at this rate full around {}", when(t))),
        Some(Full::AfterReset) => parts.push("at this rate lasts until reset".to_owned()),
        None => {}
    }
    if let Some(c) = w.calibration {
        let spread = c
            .spread
            .map_or_else(|| "n/a".to_owned(), |s| format!("{:.0} %", 100.0 * s));
        parts.push(format!(
            "calibration {:.1} % per USD from {} snapshots, spread {spread}",
            c.k, c.points
        ));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn forecast_text(f: Option<&Forecast>) -> String {
    match f {
        Some(Forecast::RunsOut { t }) => format!("runs out around {}", when(*t)),
        Some(Forecast::Unused { pct }) => format!("ends at about {pct:.0}% unused"),
        None => "none yet: needs a snapshot of the weekly window".to_owned(),
    }
}

fn five_hour_text(d: &FiveHourDay) -> String {
    let mut parts = vec![match d.expected_windows {
        Some(n) => format!("{}: about {n:.1} windows a day", d.weekday),
        None => format!("{}: no history for this day", d.weekday),
    }];
    parts.push(format!("{} started today", d.started_today));
    if let (Some(peak), Some(source)) = (d.typical_peak, d.peak_source) {
        parts.push(format!("typical peak {peak:.0}% ({})", peak_text(source)));
    }
    parts.join(", ")
}

fn peak_text(source: PeakSource) -> String {
    match source {
        PeakSource::Snapshots { n } => format!("from {n} snapshots"),
        PeakSource::Cost { n } => format!("from the cost of {n} windows"),
    }
}

fn basis_text(s: &LimitStatus) -> String {
    format!(
        "{:.1} weeks of local history ({} hours), {:.1} in weekly windows; pattern {}",
        s.basis.weeks,
        s.basis.hours,
        s.basis.mapped_weeks,
        s.basis.pattern.label()
    )
}
