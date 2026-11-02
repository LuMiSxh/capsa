// SPDX-License-Identifier: MPL-2.0
//! `census limits`: the subscription limit windows as they stand, the weekly forecast and the
//! usage pattern behind it.

use crate::now_secs;
use crate::out::Out;
use anyhow::Result;
use capsa_core::census_snapshot::Window;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use census::config::{Config, Mode};
use census::limits::{
    self, FiveHourDay, Forecast, Full, LimitStatus, MonthStatus, PeakSource, Reading, WindowStatus,
    when,
};
use serde::Serialize;

/// Prints the limit state, or the month summary in budget mode. Exit 1 when the ledger and the
/// snapshot file hold nothing.
pub fn run(out: &Out) -> Result<Outcome> {
    let config = Config::load_from(&Config::path())?;
    if config.mode == Mode::Budget {
        return run_budget(&config, out);
    }
    let status = limits::status(now_secs())?;
    if out.format == Format::Json {
        println!("{}", serde_json::to_string(&status)?);
    } else if status.data {
        print_text(&status, out);
    } else {
        println!("no usage data yet: no transcript and no limit snapshot to read");
    }
    Ok(if status.data {
        Outcome::Found
    } else {
        Outcome::Empty
    })
}

#[derive(Serialize)]
struct BudgetOut<'a> {
    mode: &'static str,
    #[serde(flatten)]
    month: &'a MonthStatus,
}

fn run_budget(config: &Config, out: &Out) -> Result<Outcome> {
    let month = limits::month(now_secs(), config.monthly_budget_usd, None)?;
    if out.format == Format::Json {
        let body = BudgetOut {
            mode: "budget",
            month: &month,
        };
        println!("{}", serde_json::to_string(&body)?);
    } else if month.data {
        print_month(&month, out);
    } else {
        println!("no usage data yet: no transcript to read");
    }
    Ok(if month.data {
        Outcome::Found
    } else {
        Outcome::Empty
    })
}

fn print_month(m: &MonthStatus, out: &Out) {
    let spent = match (m.budget_usd, m.percent) {
        (Some(budget), Some(percent)) => {
            format!("${:.2} of ${budget:.2} ({percent:.0} %)", m.spent_usd)
        }
        _ => format!(
            "${:.2}; no budget set: census config set monthly-budget <usd>",
            m.spent_usd
        ),
    };
    println!("{}", out.field("mode", "budget"));
    println!("{}", out.field("spent", &spent));
    let verdict = match m.budget_usd {
        Some(budget) if m.projected_over => {
            format!(", ${:.2} over the budget", m.projected_usd - budget)
        }
        Some(_) => ", within the budget".to_owned(),
        None => String::new(),
    };
    let projected = format!("${:.2} at the month-to-date rate{verdict}", m.projected_usd);
    println!("{}", out.field("projected", &projected));
    if let Some(per_day) = m.per_day_left_usd {
        let left = format!("${:.2} a day for {} days", per_day, m.days_left);
        println!("{}", out.field("left", &left));
    }
    let ends = format!("{} days, at {}", m.days_left, when(m.end));
    println!("{}", out.field("month ends", &ends));
}

fn print_text(s: &LimitStatus, out: &Out) {
    for w in &s.windows {
        println!("{}", out.field(label(w.window), &window_value(w)));
        if let Some(detail) = window_detail(w) {
            println!("{}", out.field("", &detail));
        }
    }
    let burn = format!(
        "${:.2}/h over 15 min, ${:.2}/h over 60 min",
        s.burn.usd_per_hour_15, s.burn.usd_per_hour_60
    );
    println!("{}", out.field("burn", &burn));
    println!(
        "{}",
        out.field("weekly", &forecast_text(s.forecast.as_ref()))
    );
    println!("{}", out.field("5h windows", &five_hour_text(&s.five_hour)));
    println!("{}", out.field("basis", &basis_text(s)));
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

/// Returns the share of a window with its source and reset, or why there is no share.
fn window_value(w: &WindowStatus) -> String {
    match (w.used, w.reading) {
        (Some(used), Some(reading)) => {
            let source = match reading {
                Reading::Snapshot { age_s } => snapshot_age(age_s),
                Reading::Estimated {
                    snapshot_age_s: Some(age),
                } => format!("estimated, {}", snapshot_age(age)),
                Reading::Estimated {
                    snapshot_age_s: None,
                } => "estimated".to_owned(),
            };
            let reset = w
                .reset_at
                .map(|t| format!("; resets {}", when(t)))
                .unwrap_or_default();
            format!("{used:.0} % {source}{reset}")
        }
        _ if w.calibration.is_none() => "no calibration yet: needs a snapshot".to_owned(),
        _ => "no window running".to_owned(),
    }
}

/// Returns the rate and the calibration of a window, when there are any.
fn window_detail(w: &WindowStatus) -> Option<String> {
    let mut parts = Vec::new();
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
        Some(Forecast::Unused { pct }) => format!("ends at about {pct:.0} % unused"),
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
        parts.push(format!("typical peak {peak:.0} % ({})", peak_text(source)));
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
