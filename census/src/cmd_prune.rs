// SPDX-License-Identifier: MPL-2.0
//! `census prune`: deletes hourly aggregates and snapshots older than a date. Raw ledger rows are
//! never pruned, since the open usage windows need them.

use crate::now_secs;
use crate::out::Out;
use anyhow::{Result, bail};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use census::config::{Config, Mode};
use census::ledger::{Ledger, Opts};
use census::limits::month_start;
use census::usage::parse_date;
use serde::Serialize;
use std::io::Write;

#[derive(clap::Args)]
pub struct Args {
    /// Remove what is older than this UTC date.
    #[arg(
        long,
        value_name = "YYYY-MM-DD",
        conflicts_with = "keep",
        required_unless_present = "keep"
    )]
    before: Option<String>,
    /// Remove what is older than this many days.
    #[arg(long, value_name = "DAYS", conflicts_with = "before")]
    keep: Option<u64>,
    /// Count what would be removed without removing it.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Serialize)]
struct PruneOut {
    older_than: String,
    dry_run: bool,
    hourly_removed: usize,
    snapshots_removed: usize,
}

/// Keeps the current month's hourly aggregates in budget mode, since the month's spend is read
/// from them. A cutoff after the month start moves back to the start, with a note.
fn keep_current_month(cutoff: u64, now: u64) -> u64 {
    let start = month_start(now);
    if Config::load().mode != Mode::Budget || cutoff <= start {
        return cutoff;
    }
    let _ = writeln!(
        std::io::stderr(),
        "note: budget mode keeps this month's hourly aggregates; the cutoff stops at the month start"
    );
    start
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let now = now_secs();
    let (cutoff, older_than) = match (a.before, a.keep) {
        (Some(date), None) => {
            let Some(ms) = parse_date(&date) else {
                bail!("--before takes a UTC date as YYYY-MM-DD, not `{date}`");
            };
            (ms / 1000, date)
        }
        (None, Some(days)) => {
            if days == 0 {
                bail!("--keep takes at least 1 day");
            }
            (
                now.saturating_sub(days.saturating_mul(86_400)),
                format!("{days} days"),
            )
        }
        _ => bail!("give exactly one of --before YYYY-MM-DD or --keep DAYS"),
    };
    let cutoff = keep_current_month(cutoff, now);
    let mut ledger = Ledger::open_and_update(&Opts::defaults(now))?;
    let report = ledger.prune(cutoff, a.dry_run)?;
    if out.format == Format::Json {
        let body = PruneOut {
            older_than,
            dry_run: a.dry_run,
            hourly_removed: report.hourly_removed,
            snapshots_removed: report.snapshots_removed,
        };
        println!("{}", serde_json::to_string(&body)?);
    } else {
        let verb = if a.dry_run { "would remove" } else { "removed" };
        println!(
            "{verb} {} hourly aggregates and {} snapshots older than {older_than}",
            report.hourly_removed, report.snapshots_removed
        );
    }
    let removed = report.hourly_removed + report.snapshots_removed;
    Ok(if removed == 0 {
        Outcome::Empty
    } else {
        Outcome::Found
    })
}
