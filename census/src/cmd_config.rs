// SPDX-License-Identifier: MPL-2.0
//! `census self config`: shows and changes the limit mode, the monthly budget and the daily limit.

use crate::cmd_limits::today_row;
use crate::now_secs;
use crate::out::Out;
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::render::{Doc, Render, emit};
use census::config::{Config, Mode};
use census::limits::{self, DailyStatus};
use serde::Serialize;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Print the mode, the monthly budget and the config file.
    Show,
    /// Change a setting: `set mode <plan|budget>`, `set monthly-budget <usd>` or `set daily-limit <usd|auto>`.
    Set(SetArgs),
}

#[derive(clap::Args)]
struct SetArgs {
    #[command(subcommand)]
    key: Key,
}

#[derive(clap::Subcommand)]
enum Key {
    /// Report the subscription windows (plan) or the spend of the month (budget).
    Mode {
        #[arg(value_enum)]
        mode: ModeArg,
    },
    /// The budget of one calendar month in USD, for budget mode.
    MonthlyBudget {
        #[arg(value_name = "USD", value_parser = positive_usd)]
        usd: f64,
    },
    /// The daily limit in USD, or `auto` to derive it from the monthly budget again.
    DailyLimit {
        #[arg(value_name = "USD|auto", value_parser = daily_arg)]
        value: DailyArg,
    },
}

#[derive(Clone, Copy)]
enum DailyArg {
    Auto,
    Usd(f64),
}

impl DailyArg {
    fn usd(self) -> Option<f64> {
        match self {
            DailyArg::Auto => None,
            DailyArg::Usd(v) => Some(v),
        }
    }
}

fn daily_arg(s: &str) -> Result<DailyArg, String> {
    if s == "auto" {
        return Ok(DailyArg::Auto);
    }
    positive_usd(s)
        .map(DailyArg::Usd)
        .map_err(|_| format!("`{s}` is not an amount above 0 or auto"))
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ModeArg {
    Plan,
    Budget,
}

impl From<ModeArg> for Mode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Plan => Mode::Plan,
            ModeArg::Budget => Mode::Budget,
        }
    }
}

fn positive_usd(s: &str) -> Result<f64, String> {
    s.trim_start_matches('$')
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| format!("`{s}` is not an amount above 0"))
}

#[derive(Serialize)]
struct ConfigOut<'a> {
    #[serde(flatten)]
    config: &'a Config,
    path: String,
    today: Option<DailyStatus>,
}

impl Render for ConfigOut<'_> {
    fn doc(&self, doc: &mut Doc) {
        let c = self.config;
        doc.title("census self config");
        doc.row("mode").text(c.mode.name());
        doc.row("monthly").text(c.monthly_budget_usd.map_or_else(
            || "not set: census self config set monthly-budget <usd>".to_owned(),
            |b| format!("${b:.2}"),
        ));
        doc.row("daily").text(c.daily_limit_usd.map_or_else(
            || "auto, derived from the monthly budget".to_owned(),
            |v| {
                format!("${v:.2} fixed; `census self config set daily-limit auto` derives it again")
            },
        ));
        let thresholds: Vec<String> = c
            .daily_thresholds
            .iter()
            .map(|t| format!("{t} %"))
            .collect();
        doc.row("thresholds").text(thresholds.join(", "));
        if let Some(d) = self.today.as_ref() {
            today_row(doc, d);
        }
        doc.row("file").dim(self.path.as_str());
    }
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let path = Config::path();
    let mut config = Config::load_from(&path)?;
    if let Action::Set(set) = a.action {
        match set.key {
            Key::Mode { mode } => config.mode = mode.into(),
            Key::MonthlyBudget { usd } => config.monthly_budget_usd = Some(usd),
            Key::DailyLimit { value } => config.daily_limit_usd = value.usd(),
        }
        config.save_to(&path)?;
    }
    let today = today(&config)?;
    let body = ConfigOut {
        config: &config,
        path: path.display().to_string(),
        today,
    };
    emit(&body, out.format)
}

/// Returns today's spend against its daily limit in budget mode; `None` in plan mode or when no
/// limit applies.
fn today(config: &Config) -> Result<Option<DailyStatus>> {
    if config.mode != Mode::Budget {
        return Ok(None);
    }
    let month = limits::month(now_secs(), config.monthly_budget_usd, None)?;
    Ok(DailyStatus::of(&month, config.daily_limit_usd))
}
