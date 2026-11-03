// SPDX-License-Identifier: MPL-2.0
//! `census self config`: shows and changes the limit mode, the monthly budget and the daily limit.

use crate::cmd_limits::today_row;
use crate::now_secs;
use crate::out::Out;
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::hint::Hinted;
use capsa_core::render::{Doc, Render, emit};
use census::config::{Billing, Config, Mode};
use census::limits::{self, DailyStatus};
use census::usage::knows_provider;
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
    /// Change a setting: `set mode <plan|budget>`, `set monthly-budget <usd>`,
    /// `set daily-limit <usd|auto>`, `set prices-refresh-days <days>` or
    /// `set billing <provider> <subscription|api>`.
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
    /// Days before the cached price list is fetched again, at least 1.
    PricesRefreshDays {
        #[arg(value_name = "DAYS", value_parser = clap::value_parser!(u64).range(1..))]
        days: u64,
    },
    /// How a provider bills its requests; a provider not set is API spend.
    Billing {
        #[arg(value_name = "PROVIDER")]
        provider: String,
        #[arg(value_name = "subscription|api")]
        value: String,
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

/// Reads the arguments of `set billing`: a provider is written in lowercase letters, digits, `-`
/// or `_`, and a billing is `subscription` or `api`.
fn billing_key(provider: &str, value: &str) -> Result<(String, Billing), Hinted> {
    let well_formed = !provider.is_empty()
        && provider
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !well_formed {
        return Err(Hinted::new(
            format!("`{provider}` is not a provider name"),
            "write the provider in lowercase, as the price table names it, such as `openai`",
        ));
    }
    let billing = match value {
        "subscription" => Billing::Subscription,
        "api" => Billing::Api,
        _ => {
            return Err(Hinted::new(
                format!("`{value}` is not a billing"),
                "use `subscription` or `api`, as in `set billing openai subscription`",
            ));
        }
    };
    Ok((provider.to_owned(), billing))
}

#[derive(Serialize)]
struct ConfigOut<'a> {
    #[serde(flatten)]
    config: &'a Config,
    path: String,
    today: Option<DailyStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
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
        doc.row("prices")
            .text(format!("refresh every {} days", c.prices_refresh_days));
        let billing: Vec<String> = c
            .billing
            .iter()
            .map(|(provider, billing)| format!("{provider} {}", billing.name()))
            .collect();
        doc.row("billing").text(if billing.is_empty() {
            "every provider is api".to_owned()
        } else {
            billing.join(", ")
        });
        for warning in &self.warnings {
            doc.row("warning").text(warning.as_str());
        }
        if let Some(d) = self.today.as_ref() {
            today_row(doc, d);
        }
        doc.row("file").dim(self.path.as_str());
    }
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let path = Config::path();
    let mut config = Config::load_from(&path)?;
    let mut warnings = Vec::new();
    if let Action::Set(set) = a.action {
        match set.key {
            Key::Mode { mode } => config.mode = mode.into(),
            Key::MonthlyBudget { usd } => config.monthly_budget_usd = Some(usd),
            Key::DailyLimit { value } => config.daily_limit_usd = value.usd(),
            Key::PricesRefreshDays { days } => config.prices_refresh_days = days,
            Key::Billing { provider, value } => {
                let (provider, billing) = billing_key(&provider, &value)?;
                if !knows_provider(&provider) {
                    warnings.push(format!(
                        "`{provider}` is not in the price table; the setting has no effect yet"
                    ));
                }
                config.billing.insert(provider, billing);
            }
        }
        config.save_to(&path)?;
    }
    let today = today(&config)?;
    let body = ConfigOut {
        config: &config,
        path: path.display().to_string(),
        today,
        warnings,
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
