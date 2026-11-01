// SPDX-License-Identifier: MPL-2.0
//! The `census` binary: parses the command line and dispatches to the `cmd_*` modules.

mod cmd_context;
mod cmd_limits;
mod cmd_prune;
mod cmd_session;
mod cmd_statusline;
mod cmd_usage;
mod out;
mod usage_views;

use anyhow::Result;
use capsa_core::exit::{Outcome, finish, report, reset_sigpipe};
use capsa_core::format::{Format, resolve};
use clap::{Parser, Subcommand};
use std::convert::Infallible;
use std::process::ExitCode;

/// Usage analytics for coding-agent sessions. Exit: 0 results, 1 nothing found, 2 error.
#[derive(Parser)]
#[command(name = "census", version)]
struct Cli {
    /// Output format: agent, human or json. Agent unless CAPSA_HUMAN=1 selects human.
    #[arg(long, value_enum, global = true)]
    format: Option<Format>,
    /// Shorthand for --format json.
    #[arg(long, global = true)]
    json: bool,
    /// Shorthand for --format human.
    #[arg(long, global = true)]
    human: bool,
    /// Shorthand for --format agent.
    #[arg(long, global = true)]
    agent: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Token, cost and tool-call statistics from Claude Code and Codex transcripts.
    Usage(cmd_usage::Args),
    /// Context size of the current session against its auto-compaction threshold.
    Context(cmd_context::Args),
    /// Subscription limit windows: estimates, burn rate and the weekly forecast.
    Limits,
    /// Summary of the current session: duration, cost, spawns per role, context and compactions.
    Session(cmd_session::Args),
    /// Records the limit snapshots of the statusLine input; `--line` prints the status line.
    Statusline(cmd_statusline::Args),
    /// Deletes hourly aggregates and snapshots older than a date; raw ledger rows are kept.
    Prune(cmd_prune::Args),
}

impl Cli {
    /// The output format: an explicit flag wins (json, human, agent), then `CAPSA_HUMAN`.
    fn format(&self) -> Format {
        resolve(
            self.human || self.format == Some(Format::Human),
            self.json || self.format == Some(Format::Json),
            self.agent || self.format == Some(Format::Agent),
        )
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
    }
}

/// Seconds since the Unix epoch, or 0 when the clock is before it.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Clap value parser for path arguments: `~` and `~/…` become $HOME, as a shell would expand them.
fn home<T: From<String>>(arg: &str) -> Result<T, Infallible> {
    Ok(T::from(capsa_core::paths::expand_home(arg)))
}

/// The error for a command line clap rejected, as one message.
fn parse_error(e: &clap::Error) -> anyhow::Error {
    anyhow::Error::msg(
        e.to_string()
            .trim()
            .trim_start_matches("error: ")
            .to_owned(),
    )
}

/// The format of a command line clap could not parse, read from its raw flags.
fn raw_format(args: &[String]) -> Format {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value = args.iter().enumerate().find_map(|(i, a)| {
        a.strip_prefix("--format=")
            .or_else(|| (a == "--format").then(|| args.get(i + 1).map_or("", String::as_str)))
    });
    resolve(
        has("--human") || value == Some("human"),
        has("--json") || value == Some("json"),
        has("--agent") || value == Some("agent"),
    )
}

fn main() -> ExitCode {
    // `census … | head` should end quietly like other CLI tools, not panic on a closed pipe.
    reset_sigpipe();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // Help and version are output, not errors.
        Err(e) if !e.use_stderr() => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let args: Vec<String> = std::env::args().collect();
            return report(&parse_error(&e), raw_format(&args));
        }
    };
    let format = cli.format();
    finish(execute(cli, format), format)
}
