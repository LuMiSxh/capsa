// SPDX-License-Identifier: MPL-2.0
//! The `census` binary: parses the command line and dispatches to the `cmd_*` modules.

mod cmd_config;
mod cmd_context;
mod cmd_doctor;
mod cmd_hook;
mod cmd_limits;
mod cmd_prune;
mod cmd_self;
mod cmd_session;
mod cmd_statusline;
mod cmd_usage;
mod out;
mod setup;
mod usage_views;

use anyhow::Result;
use capsa_core::cli::{App, FormatArgs};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Usage analytics for coding-agent sessions. Exit: 0 results, 1 nothing found, 2 error.
#[derive(Parser)]
#[command(name = "census", version)]
struct Cli {
    #[command(flatten)]
    output: FormatArgs,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Token, cost and tool-call statistics from Claude Code and Codex transcripts.
    Usage(cmd_usage::Args),
    /// Context size of the current session against its auto-compaction threshold.
    Context(cmd_context::Args),
    /// Subscription limit windows (plan mode) or the spend of the month (budget mode).
    Limits,

    /// Summary of the current session: duration, cost, spawns per role, context and compactions.
    Session(cmd_session::Args),
    /// Records the limit snapshots of the statusLine input; `--line` prints the status line.
    Statusline(cmd_statusline::Args),
    /// Deletes hourly aggregates and snapshots older than a date; raw ledger rows are kept.
    Prune(cmd_prune::Args),
    /// Claude Code hook for UserPromptSubmit and PostToolUse: prints context and limit levels the main agent has not heard yet. Always exits 0.
    Hook,
    /// Installation and health of census: `self skill|setup|config|doctor|update|uninstall`.
    #[command(name = "self")]
    Itself(cmd_self::Args),
}

impl App for Cli {
    fn format(&self) -> Format {
        self.output.resolve()
    }
}

fn execute(cli: Cli, format: Format) -> Result<Outcome> {
    let out = out::Out::new(format);
    match cli.cmd {
        Cmd::Usage(a) => cmd_usage::run(a, &out),
        Cmd::Context(a) => cmd_context::run(&a, &out),
        Cmd::Limits => cmd_limits::run(&out),

        Cmd::Session(a) => cmd_session::run(&a, &out),
        Cmd::Statusline(a) => Ok(cmd_statusline::run(&a)),
        Cmd::Prune(a) => cmd_prune::run(a, &out),
        Cmd::Hook => Ok(cmd_hook::run()),
        Cmd::Itself(a) => cmd_self::run(a, &out),
    }
}

/// Seconds since the Unix epoch, or 0 when the clock is before it. `CENSUS_NOW` replaces the clock
/// so that the tests can fix it. `CENSUS_TZ_OFFSET_SECS` fixes the local offset (see `limits::clock`).
/// `CENSUS_HOOK_DEADLINE_MS` replaces the hook's limits deadline (see `cmd_hook`).
fn now_secs() -> u64 {
    std::env::var("CENSUS_NOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        })
}

fn main() -> ExitCode {
    capsa_core::cli::main::<Cli>(execute)
}
