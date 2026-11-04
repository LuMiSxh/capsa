// SPDX-License-Identifier: MPL-2.0
//! The `vicarius` binary: parses the command line and dispatches to the `cmd_*` modules.

mod cmd_config;
mod cmd_doctor;
mod cmd_hook;
mod cmd_self;
mod cmd_serve;
mod cmd_service;
mod cmd_status;
mod service;
mod status_client;
#[cfg(test)]
mod test_support;

use anyhow::Result;
use capsa_core::cli::{App, FormatArgs};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Routes subagents to DeepSeek while credit lasts. Exit: 0 result, 1 nothing found, 2 error.
#[derive(Parser)]
#[command(name = "vicarius", version)]
struct Cli {
    #[command(flatten)]
    output: FormatArgs,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the proxy in the foreground until SIGTERM or SIGINT.
    Serve(cmd_serve::Args),
    /// Show the service: the gate, the balance, the refresh, the requests, the tokens and the agents.
    Status,
    /// Run a Claude Code hook: reads stdin and prints the DeepSeek agent line on SessionStart.
    Hook,
    /// Settings and installation of vicarius: `self config`, `self update` and `self uninstall`.
    #[command(name = "self")]
    Itself(cmd_self::Args),
}

impl App for Cli {
    fn format(&self) -> Format {
        self.output.resolve()
    }
}

fn execute(cli: Cli, format: Format) -> Result<Outcome> {
    match cli.cmd {
        Cmd::Serve(a) => cmd_serve::run(&a, format),
        Cmd::Status => cmd_status::run(format),
        Cmd::Hook => Ok(cmd_hook::run()),
        Cmd::Itself(a) => cmd_self::run(a, format),
    }
}

fn main() -> ExitCode {
    capsa_core::cli::main::<Cli>(execute)
}
