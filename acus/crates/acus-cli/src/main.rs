// SPDX-License-Identifier: MPL-2.0
//! The `acus` binary: parses the command line and dispatches to the `cmd_*` modules.

mod agents;
mod cmd_blame;
mod cmd_ctx;
mod cmd_decide;
mod cmd_diff;
mod cmd_files;
mod cmd_find;
mod cmd_follow;
mod cmd_guard;

mod cmd_log;
mod cmd_map;
mod cmd_outline;
mod cmd_patch;
mod cmd_run;
mod cmd_self;
mod cmd_show;
mod cmd_skill;
mod cmd_uninstall;
mod cmd_update;
mod config;

mod fmt;
mod git;
mod globs;
mod source;

use anyhow::{Result, bail};
use capsa_core::exit::{Outcome, finish, report, reset_sigpipe};
use capsa_core::format::{Format, resolve};
use clap::{Parser, Subcommand};
use std::convert::Infallible;
use std::process::ExitCode;

/// This binary as the self-update and uninstall code sees it.
#[cfg(feature = "cmd-update")]
pub const TOOL: capsa_core::update::Tool = capsa_core::update::Tool {
    name: "acus",
    version: env!("CARGO_PKG_VERSION"),
};

/// Agent-first code search, outline and editing. Exit: 0 results, 1 nothing found, 2 error.
#[derive(Parser)]
#[command(name = "acus", version)]
pub struct Cli {
    /// Output format: agent, human or json. Agent unless CAPSA_HUMAN=1 selects human.
    #[arg(long, value_enum, global = true)]
    format: Option<Format>,
    /// Shorthand for --format json.
    #[arg(long, global = true)]
    json: bool,
    /// Shorthand for --format human.
    #[arg(long, global = true)]
    human: bool,
    /// Shorthand for --format agent. Not global: `skill --agent NAME` is a different option.
    #[arg(long)]
    agent: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Regex search; hits grouped by file and enclosing symbol.
    Find(cmd_find::Args),
    /// Symbols with kinds and line ranges; directories are walked.
    Outline(cmd_outline::Args),
    /// Directory tree with file and line counts, opened as far as a line budget allows.
    Map(cmd_map::Args),
    /// Files under PATH… (default `.`), sorted, one per line; `-g`, `-t`, `--hidden` and `-u` as in `find`; `--lines` adds each file's line count and a total.
    Files(cmd_files::Args),
    /// Print files, line ranges or symbols with line numbers; `REV:path` reads a file as of a
    /// git revision.
    Show(cmd_show::Args),
    /// Who last changed each run of lines of a symbol or range, one line per commit.
    Blame(cmd_blame::Args),
    /// Who calls a symbol, recursively; `--down` lists what it calls.
    ///
    /// Syntactic only: calls are matched by name in the source, so dynamic and trait dispatch,
    /// function references passed as values and implicit calls (Drop, operators, getters) are
    /// missed. A `?` edge matched by name alone; it is shown but never followed.
    Follow(cmd_follow::Args),
    /// Apply a patch from stdin: SEARCH/REPLACE blocks under `*** Update File:`, plus `*** Add File`,
    /// `*** Delete File`, `*** Copy File`, `*** Replace Symbol`, `*** Insert Before/After`,
    /// `*** Delete Symbol`, `*** Move Symbol` and `*** Replace All` (`acus patch --help` has the syntax).
    Patch(cmd_patch::Args),

    /// Yes/no, choice or score judgement via a Jev-compatible API (needs `cmd-decide`).
    Decide(cmd_decide::Args),
    /// Run a shell command; print its output plus the code its `path:line` references point at.
    Ctx(cmd_ctx::Args),
    /// Uncommitted (or `REV`, `A..B`) changes grouped by enclosing symbol; `-p` adds the lines.
    Diff(cmd_diff::Args),
    /// Recent commits, one line each with date, author and changed lines.
    Log(cmd_log::Args),
    /// Run several commands from stdin, one per line, or as a JSON array of argument lists.
    Run(cmd_run::Args),
    /// PreToolUse hook for Claude Code and Codex: rewrites Bash, PowerShell, Read, Grep and Glob calls
    /// into the acus command that does the same work, or adds a hint or refusal that names it.
    Guard(cmd_guard::Args),
    /// Installation and health of acus itself: `self update`, `self skill`, `self uninstall`.
    #[command(name = "self")]
    Itself(cmd_self::Args),
}

impl Cmd {
    fn name(&self) -> &'static str {
        match self {
            Cmd::Find(_) => "find",
            Cmd::Outline(_) => "outline",
            Cmd::Map(_) => "map",
            Cmd::Files(_) => "files",
            Cmd::Show(_) => "show",
            Cmd::Blame(_) => "blame",
            Cmd::Follow(_) => "follow",
            Cmd::Patch(_) => "patch",

            Cmd::Decide(_) => "decide",
            Cmd::Run(_) => "run",
            Cmd::Ctx(_) => "ctx",
            Cmd::Diff(_) => "diff",
            Cmd::Log(_) => "log",
            Cmd::Guard(_) => "guard",
            Cmd::Itself(a) => a.name(),
        }
    }

    /// The outcome for which `--json` prints `null` (no results) instead of nothing.
    fn json_null_on(&self) -> Option<Outcome> {
        match self {
            Cmd::Find(_) | Cmd::Log(_) | Cmd::Files(_) => Some(Outcome::Empty),
            _ => None,
        }
    }
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

/// Runs one parsed command line in `format`; `run` calls this for every batch entry.
pub fn execute(cli: Cli, format: Format, nested: bool) -> Result<Outcome> {
    let cfg = config::load()?;
    let name = cli.cmd.name();
    let null_on = cli.cmd.json_null_on();
    if cfg.commands.disabled.iter().any(|d| d == name) {
        return Err(capsa_core::hint::Hinted::new(
            format!("acus {name} is disabled by configuration"),
            format!(
                "remove it from [commands] disabled in {} or from ACUS_DISABLE",
                cfg.path.display()
            ),
        )
        .into());
    }
    let out = fmt::Out::new(format);
    let result = match cli.cmd {
        Cmd::Find(a) => cmd_find::run(&a, &out),
        Cmd::Outline(a) => cmd_outline::run(a, &out),
        Cmd::Map(a) => cmd_map::run(a),
        Cmd::Files(a) => cmd_files::run(a, &out),
        Cmd::Show(a) => cmd_show::run(a, &out),
        Cmd::Blame(a) => cmd_blame::run(a, &out),
        Cmd::Follow(a) => cmd_follow::run(a, &out),
        Cmd::Patch(a) => cmd_patch::run(a, &out, nested, &cfg),

        Cmd::Decide(a) => cmd_decide::run(a, &out, &cfg),
        Cmd::Run(_) if nested => bail!("`run` cannot be nested"),
        Cmd::Run(a) => cmd_run::run(a, &out),
        Cmd::Ctx(a) => cmd_ctx::run(a, &out, &cfg),
        Cmd::Diff(a) => cmd_diff::run(a, &out),
        Cmd::Log(a) => cmd_log::run(a, &out),
        Cmd::Guard(a) => cmd_guard::run(a, &cfg),
        Cmd::Itself(a) => cmd_self::run(a, &cfg),
    }?;
    if !nested && out.format == Format::Json && null_on.is_some_and(|o| o == result) {
        println!("null");
    }
    Ok(result)
}

/// Clap value parser for path arguments: `~` and `~/…` become $HOME, as a shell would expand
/// them.
pub fn home<T: From<String>>(arg: &str) -> Result<T, Infallible> {
    Ok(T::from(capsa_core::paths::expand_home(arg)))
}

/// A hint for a grep or rg flag that `acus find` lacks, printed after clap's own error; `args`
/// is the whole command line, `acus` first.
pub fn find_flag_hint(e: &clap::Error, args: &[String]) -> Option<String> {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    let sub = args.iter().skip(1).find(|a| !a.starts_with('-'));
    if e.kind() != ErrorKind::UnknownArgument || sub.map(String::as_str) != Some("find") {
        return None;
    }
    let Some(ContextValue::String(flag)) = e.get(ContextKind::InvalidArg) else {
        return None;
    };
    let flag = flag.split('=').next().unwrap_or(flag);
    let near = match flag {
        "-s" | "-S" | "--smart-case" | "--no-messages" => "`-i` ignores case",
        "--files-without-match" => "`acus find -l PAT` lists the files that match",
        "--files" => return Some("use acus files [-g GLOB] [PATH] to list files".into()),
        "--no-heading" | "--color" | "--no-line-number" | "--heading" | "--line-number" => {
            "output is always grouped by file with line numbers; drop the flag"
        }
        _ => "that flag does not exist",
    };
    Some(format!(
        "{near}; every flag is listed by `acus find --help`"
    ))
}

/// The error for a command line clap rejected; the find flag hint becomes its `Hint`.
pub fn parse_error(e: &clap::Error, args: &[String]) -> anyhow::Error {
    let Some(hint) = find_flag_hint(e, args) else {
        let message = e.to_string();
        return anyhow::Error::msg(message.trim().trim_start_matches("error: ").to_owned());
    };
    let flag = match e.get(clap::error::ContextKind::InvalidArg) {
        Some(clap::error::ContextValue::String(f)) => f.as_str(),
        _ => "",
    };
    anyhow::Error::new(capsa_core::hint::Hinted::new(
        format!("unexpected argument '{flag}' found"),
        hint,
    ))
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
    // `acus … | head` should end quietly like other CLI tools, not panic on a closed pipe.
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
            return report(&parse_error(&e, &args), raw_format(&args));
        }
    };
    let format = cli.format();
    finish(execute(cli, format, false), format)
}
