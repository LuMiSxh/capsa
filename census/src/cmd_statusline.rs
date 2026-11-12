// SPDX-License-Identifier: MPL-2.0
//! `census statusline`: records the limit snapshots of Claude Code's statusLine input, and prints
//! the status line with `--line`.

use capsa_core::census_snapshot;
use capsa_core::exit::Outcome;
use census::config::{Config, Mode};
use census::{interop, limits, statusline};
use std::io::Read;

#[derive(clap::Args)]
pub struct Args {
    /// Prints the status line: context against the compaction threshold, then the limit windows.
    #[arg(long)]
    line: bool,
}

/// Always returns exit 0. The input is read from stdin; an unreadable input or an unwritable
/// snapshot file loses only the part it affects, and nothing is printed for it.
pub fn run(a: &Args) -> Outcome {
    let mut text = String::new();
    let _ = std::io::stdin().read_to_string(&mut text);
    let Some(input) = statusline::parse(&text) else {
        return Outcome::Found;
    };
    let snapshots = statusline::snapshots(&input, crate::now_secs());
    let _ = statusline::record(&census_snapshot::path(), &snapshots);
    if a.line {
        let config = Config::load();
        let month = (config.mode == Mode::Budget)
            .then(|| limits::month_stored(crate::now_secs(), config.monthly_budget_usd).ok())
            .flatten();
        let segments = interop::segments(input.session_id.as_deref(), crate::now_secs());
        let line = match month {
            Some(month) => statusline::budget_line(&input, &month, segments),
            None => statusline::line(&input, &snapshots, segments),
        };
        if !line.is_empty() {
            println!("{line}");
        }
    }
    Outcome::Found
}
