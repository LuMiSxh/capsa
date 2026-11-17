// SPDX-License-Identifier: MPL-2.0
//! `census self config`: shows and changes the limit mode, the monthly budget and the daily limit.

use crate::cmd_limits::today_row;
use crate::now_secs;
use crate::out::Out;
use anyhow::Result;
use capsa_core::config::ConfigFile;
use capsa_core::exit::Outcome;
use capsa_core::hint::{ResultExt, hinted};
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
    /// `set daily-limit <usd|auto>`, `set prices-refresh-days <days>`,
    /// `set billing <provider> <subscription|api>`, or a comma-separated list of percents for
    /// `set daily-thresholds`, `set five-hour-thresholds` or `set weekly-thresholds`.
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
    /// Percents of the daily limit at which the hook announces, such as `50,80,100`.
    DailyThresholds {
        #[arg(value_name = "PCT,PCT,...")]
        list: String,
    },
    /// Percents of the five-hour window the hook announces, from 1 to 100, such as `85,95`.
    FiveHourThresholds {
        #[arg(value_name = "PCT,PCT,...")]
        list: String,
    },
    /// Percents of the weekly window the hook announces, from 1 to 100, such as `90,95`.
    WeeklyThresholds {
        #[arg(value_name = "PCT,PCT,...")]
        list: String,
    },
}

#[derive(Clone, Copy)]
enum DailyArg {
    Auto,
    Usd(f64),
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
fn billing_key(provider: &str, value: &str) -> Result<(String, Billing)> {
    let well_formed = !provider.is_empty()
        && provider
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !well_formed {
        return Err(hinted(
            format!("`{provider}` is not a provider name"),
            "write the provider in lowercase, as the price table names it, such as `openai`",
        ));
    }
    let billing = match value {
        "subscription" => Billing::Subscription,
        "api" => Billing::Api,
        _ => {
            return Err(hinted(
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
        doc.row("daily thresholds")
            .text(percent_list(&c.daily_thresholds));
        doc.row("5h thresholds")
            .text(percent_list(&c.five_hour_thresholds));
        doc.row("weekly thresholds")
            .text(percent_list(&c.weekly_thresholds));
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

/// Returns the percentages as `85 %, 96 %`.
fn percent_list(values: &[u32]) -> String {
    let parts: Vec<String> = values.iter().map(|t| format!("{t} %")).collect();
    parts.join(", ")
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let file = Config::file();
    let mut warnings = Vec::new();
    if let Action::Set(set) = a.action {
        apply(&file, set.key, &mut warnings)?;
    }
    let config = file.load::<Config>().hinted()?;
    let today = today(&config)?;
    let body = ConfigOut {
        config: &config,
        path: file.path().display().to_string(),
        today,
        warnings,
    };
    emit(&body, out.format)
}

/// Writes one `set` key to the config file. A billing is written with the whole billing table, so
/// the default `anthropic` entry stays listed in a file that does not name it.
fn apply(file: &ConfigFile, key: Key, warnings: &mut Vec<String>) -> Result<()> {
    match key {
        Key::Mode { mode } => {
            file.set::<Config>("mode", Mode::from(mode).name())
                .hinted()?;
        }
        Key::MonthlyBudget { usd } => {
            file.set::<Config>("monthly_budget_usd", &format!("{usd:?}"))
                .hinted()?;
        }
        Key::DailyLimit {
            value: DailyArg::Usd(usd),
        } => {
            file.set::<Config>("daily_limit_usd", &format!("{usd:?}"))
                .hinted()?;
        }
        Key::DailyLimit {
            value: DailyArg::Auto,
        } => {
            let mut config = file.load::<Config>().hinted()?;
            config.daily_limit_usd = None;
            file.save(&config).hinted()?;
        }
        Key::PricesRefreshDays { days } => {
            file.set::<Config>("prices_refresh_days", &days.to_string())
                .hinted()?;
        }
        Key::Billing { provider, value } => {
            let (provider, billing) = billing_key(&provider, &value)?;
            if !knows_provider(&provider) {
                warnings.push(format!(
                    "`{provider}` is not in the price table; the setting has no effect yet"
                ));
            }
            let mut config = file.load::<Config>().hinted()?;
            config.billing.insert(provider, billing);
            file.save(&config).hinted()?;
        }
        Key::DailyThresholds { list } => {
            let mut config = file.load::<Config>().hinted()?;
            config.daily_thresholds = thresholds_arg(&list, None)?;
            file.save(&config).hinted()?;
        }
        Key::FiveHourThresholds { list } => {
            let mut config = file.load::<Config>().hinted()?;
            config.five_hour_thresholds = thresholds_arg(&list, Some(100))?;
            file.save(&config).hinted()?;
        }
        Key::WeeklyThresholds { list } => {
            let mut config = file.load::<Config>().hinted()?;
            config.weekly_thresholds = thresholds_arg(&list, Some(100))?;
            file.save(&config).hinted()?;
        }
    }
    Ok(())
}

/// Reads a comma-separated list of whole percents, each from 1 to `max` or, without a maximum,
/// above 0. The list is returned sorted and without repeats.
fn thresholds_arg(list: &str, max: Option<u32>) -> Result<Vec<u32>> {
    let range = max.map_or_else(|| "above 0".to_owned(), |m| format!("from 1 to {m}"));
    if list.trim().is_empty() {
        return Err(hinted(
            "the list of percents is empty",
            format!("give whole percents {range}, separated by commas, such as `85,95`"),
        ));
    }
    let mut values = Vec::new();
    for part in list.split(',') {
        let value = part
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|v| *v > 0 && max.is_none_or(|m| *v <= m));
        let Some(value) = value else {
            return Err(hinted(
                format!("`{}` is not a whole percent {range}", part.trim()),
                format!("write whole percents {range}, separated by commas, such as `85,95`"),
            ));
        };
        values.push(value);
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

/// Returns today's spend against its daily limit in budget mode; `None` in plan mode or when no
/// limit applies.
fn today(config: &Config) -> Result<Option<DailyStatus>> {
    if config.mode != Mode::Budget {
        return Ok(None);
    }
    let month = limits::month(now_secs(), config.monthly_budget_usd, None, None)?;
    Ok(DailyStatus::of(&month, config.daily_limit_usd))
}
