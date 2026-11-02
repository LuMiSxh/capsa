// SPDX-License-Identifier: MPL-2.0
//! Owns the command line the capsa tools share: the output format flags, the `~` path parser,
//! the rendering of parse errors and the `main` skeleton that runs a tool.

#[cfg(test)]
mod tests;

use std::convert::Infallible;
use std::process::ExitCode;

use crate::exit::{Outcome, finish, report, reset_sigpipe};
use crate::format::{self, Env, Format, Process};

/// The output format flags, flattened into a tool's root command.
#[derive(clap::Args, Debug, Clone, Copy, Default)]
pub struct FormatArgs {
    /// Output format: agent, human or json. Agent unless CAPSA_HUMAN=1 selects human.
    #[arg(long, value_enum, global = true)]
    pub format: Option<Format>,
    /// Shorthand for --format json.
    #[arg(long, global = true)]
    pub json: bool,
    /// Shorthand for --format human.
    #[arg(long, global = true)]
    pub human: bool,
    /// Shorthand for --format agent.
    #[arg(long, global = true)]
    pub agent: bool,
}

impl FormatArgs {
    /// Returns the format the flags select, with `CAPSA_HUMAN=1` deciding when no flag does.
    pub fn resolve(&self) -> Format {
        self.resolve_in(&Process)
    }

    fn resolve_in(&self, env: &dyn Env) -> Format {
        format::resolve_in(
            self.human || self.format == Some(Format::Human),
            self.json || self.format == Some(Format::Json),
            self.agent || self.format == Some(Format::Agent),
            env,
        )
    }
}

/// Returns the format of a command line clap could not parse, read from its raw arguments.
pub fn raw_format(args: &[String]) -> Format {
    raw_format_in(args, &Process)
}

fn raw_format_in(args: &[String], env: &dyn Env) -> Format {
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    let value = args.iter().enumerate().find_map(|(i, arg)| {
        arg.strip_prefix("--format=")
            .or_else(|| (arg == "--format").then(|| args.get(i + 1).map_or("", String::as_str)))
    });
    format::resolve_in(
        has("--human") || value == Some("human"),
        has("--json") || value == Some("json"),
        has("--agent") || value == Some("agent"),
        env,
    )
}

/// Returns the error for a command line clap rejected, as clap's message without its `error: `.
pub fn plain_parse_error(e: &clap::Error) -> anyhow::Error {
    anyhow::Error::msg(
        e.to_string()
            .trim()
            .trim_start_matches("error: ")
            .to_owned(),
    )
}

/// Clap value parser for paths: `~` and `~/…` become the home directory, as a shell expands them.
pub fn home<T: From<String>>(arg: &str) -> Result<T, Infallible> {
    Ok(T::from(crate::paths::expand_home(arg)))
}

/// A tool's command line: how it is parsed, and how its output and errors are rendered.
pub trait App: clap::Parser {
    /// Whether SIGPIPE is reset, so that a pipe into `head` ends the tool quietly.
    const RESET_SIGPIPE: bool = true;

    /// Returns the output format the parsed command line selects.
    fn format(&self) -> Format;

    /// Returns the error for a command line clap rejected.
    fn parse_error(e: &clap::Error, _args: &[String]) -> anyhow::Error {
        plain_parse_error(e)
    }
}

/// Runs a tool: parses its command line as `A`, prints help and version on stdout, reports a
/// rejected command line in the raw format, then runs `execute` and returns its exit code.
pub fn main<A: App>(execute: impl FnOnce(A, Format) -> anyhow::Result<Outcome>) -> ExitCode {
    if A::RESET_SIGPIPE {
        reset_sigpipe();
    }
    let app = match A::try_parse() {
        Ok(app) => app,
        // Help and version are output, not errors.
        Err(e) if !e.use_stderr() => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let args: Vec<String> = std::env::args().collect();
            return report(&A::parse_error(&e, &args), raw_format(&args));
        }
    };
    let format = app.format();
    finish(execute(app, format), format)
}
