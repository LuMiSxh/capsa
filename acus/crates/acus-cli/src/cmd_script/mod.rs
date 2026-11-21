// SPDX-License-Identifier: MPL-2.0
//! `acus script`: runs a JavaScript program in-process with the acus commands bound as functions,
//! and keeps saved scripts that run by name.

mod argv;
mod bindings;
mod manage;
mod mcp;
mod recommend;
mod result;
mod shapes;
mod store;
mod usage;

use crate::config::Config;
use acus_script::{Limits, ScriptError};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::hinted;
use capsa_core::render;
use serde_json::{Map, Value};
use std::io::Read;
use std::time::Instant;

/// Returns the SessionStart line that names the repo's saved scripts; see
/// [`recommend::scripts_here`].
pub(crate) fn scripts_here(cfg: &Config, cwd: &std::path::Path) -> Option<String> {
    recommend::scripts_here(cfg, cwd, store::now())
}

#[derive(clap::Args)]
#[command(
    args_conflicts_with_subcommands = true,
    after_help = "Run a saved script with `acus script NAME [key=value...]`; `NAME@3` pins version 3."
)]
pub struct Args {
    /// The program, as the body of an async function: `await` and `return` work at its top
    /// level, and the returned value is the result. Default: read it from stdin.
    #[arg(short = 'e', long = "eval", value_name = "CODE")]
    code: Option<String>,
    #[command(subcommand)]
    saved: Option<Saved>,
}

#[derive(clap::Subcommand)]
enum Saved {
    /// Save stdin as the next version of a script; it needs `export const description`,
    /// `export const params` and `export default async function (args)`.
    Save(manage::SaveArgs),
    /// The saved scripts with scope, latest version, description, last use and use count.
    List,
    /// A script's description, parameters and source.
    Show(manage::ShowArgs),
    /// A script's versions with their dates and notes.
    Log(manage::LogArgs),
    #[command(external_subcommand)]
    Run(Vec<String>),
}

/// Runs the program or the saved-script action and renders the result. Exit: 0 a result, 1 an
/// empty one, 2 an error that names the program line.
pub fn run(a: Args, cfg: Config, format: Format) -> Result<Outcome> {
    let cwd = || std::env::current_dir().context("cannot read the current directory");
    match a.saved {
        None => run_inline(a.code, cfg, format),
        Some(Saved::Save(s)) => render::emit(&manage::save(&s, cfg)?, format),
        Some(Saved::List) => render::emit(&manage::list(&cfg, &cwd()?), format),
        Some(Saved::Show(s)) => render::emit(&manage::show(&s, &cfg, &cwd()?)?, format),
        Some(Saved::Log(l)) => render::emit(&manage::history(&l, &cfg, &cwd()?)?, format),
        Some(Saved::Run(words)) => run_saved(&words, cfg, format),
    }
}

fn run_inline(code: Option<String>, cfg: Config, format: Format) -> Result<Outcome> {
    let code = match code {
        Some(code) => code,
        None => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .context("cannot read the program from stdin")?;
            text
        }
    };
    if code.trim().is_empty() {
        return Err(hinted(
            "no program given",
            "pass it with -e 'return await find(\"x\")' or on stdin",
        ));
    }
    let limits = limits(&cfg);
    let host = bindings::Bindings::new(cfg)?;
    let servers = host.mcp();
    let outcome = acus_script::run(&code, host, &limits);
    servers.shutdown();
    let outcome = outcome.map_err(failure)?;
    render::emit(&result::Output::new(outcome), format)
}

/// Runs `NAME[@V]` with the `key=value` words as `args`, and logs the run.
fn run_saved(words: &[String], cfg: Config, format: Format) -> Result<Outcome> {
    let (spec, rest) = words.split_first().context("no script name given")?;
    let (name, version) = store::parse_spec(spec)?;
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let found = store::Store::open(&cfg, &cwd).locate(&name, version)?;
    let args = script_args(rest)?;
    let entry = format!("{}:{name}@{}", found.scope.as_str(), found.version);
    let log = usage::path(&cfg);
    let limits = limits(&cfg);
    let host = bindings::Bindings::new(cfg)?;
    let servers = host.mcp();
    let started = Instant::now();
    let outcome = acus_script::run_module(&entry, &args, host, &limits);
    servers.shutdown();
    usage::record(
        &log,
        &usage::Run {
            name,
            version: found.version,
            scope: found.scope,
            cwd: cwd.to_string_lossy().into_owned(),
            ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            ok: outcome.is_ok(),
        },
    );
    render::emit(&result::Output::new(outcome.map_err(failure)?), format)
}

/// Returns the run's limits: the defaults, with the configured time limit.
fn limits(cfg: &Config) -> Limits {
    let mut limits = Limits::default();
    if let Some(secs) = cfg.script.timeout {
        limits.deadline = std::time::Duration::from_secs(secs);
    }
    limits
}

/// Builds the `args` object from `key=value` words; a repeated key makes an array.
fn script_args(words: &[String]) -> Result<Value> {
    let mut args = Map::new();
    for word in words {
        let (key, value) = word
            .split_once('=')
            .filter(|(k, _)| !k.is_empty())
            .ok_or_else(|| {
                hinted(
                    format!("`{word}` is not key=value"),
                    "pass parameters as key=value, as in `acus script NAME pattern=todo`",
                )
            })?;
        let value = Value::String(value.to_owned());
        match args.get_mut(key) {
            Some(Value::Array(list)) => list.push(value),
            Some(first) => *first = Value::Array(vec![first.take(), value]),
            None => {
                args.insert(key.to_owned(), value);
            }
        }
    }
    Ok(Value::Object(args))
}

/// Turns a failed run into the error `exit` prints, keeping a thrown host error's hint.
fn failure(error: ScriptError) -> anyhow::Error {
    let message = error.to_string();
    match error {
        ScriptError::Uncaught {
            hint: Some(hint), ..
        } => hinted(message, hint),
        ScriptError::Timeout { .. } => hinted(
            message,
            "narrow the work, or bound a slow command with run(cmd, {timeout: SECS})",
        ),
        ScriptError::Invalid(_) => hinted(
            message,
            "a saved script exports `description`, `params` and `default async function (args)`",
        ),
        other => anyhow::Error::new(other),
    }
}
