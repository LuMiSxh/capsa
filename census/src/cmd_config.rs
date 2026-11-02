// SPDX-License-Identifier: MPL-2.0
//! `census config`: shows and changes the limit mode and the monthly budget.

use crate::out::Out;
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use census::config::{Config, Mode};
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
    /// Change a setting: `set mode <plan|budget>` or `set monthly-budget <usd>`.
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
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let path = Config::path();
    let mut config = Config::load_from(&path)?;
    if let Action::Set(set) = a.action {
        match set.key {
            Key::Mode { mode } => config.mode = mode.into(),
            Key::MonthlyBudget { usd } => config.monthly_budget_usd = Some(usd),
        }
        config.save_to(&path)?;
    }
    print(&config, &path, out)?;
    Ok(Outcome::Found)
}

fn print(config: &Config, path: &std::path::Path, out: &Out) -> Result<()> {
    let path = path.display().to_string();
    if out.format == Format::Json {
        println!("{}", serde_json::to_string(&ConfigOut { config, path })?);
        return Ok(());
    }
    println!("{}", out.field("mode", config.mode.name()));
    let budget = config.monthly_budget_usd.map_or_else(
        || "not set: census config set monthly-budget <usd>".to_owned(),
        |b| format!("${b:.2}"),
    );
    println!("{}", out.field("monthly", &budget));
    println!("{}", out.field("file", &path));
    Ok(())
}
