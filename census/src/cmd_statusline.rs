// SPDX-License-Identifier: MPL-2.0
//! `census statusline`: records the limit snapshots of Claude Code's statusLine input, and prints
//! the status line with `--line`.

use capsa_core::census_snapshot;
use capsa_core::exit::Outcome;
use census::statusline;
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
        let line = statusline::line(&input, &snapshots);
        if !line.is_empty() {
            println!("{line}");
        }
    }
    Outcome::Found
}
