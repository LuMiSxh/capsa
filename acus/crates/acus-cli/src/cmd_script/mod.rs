// SPDX-License-Identifier: MPL-2.0
//! `acus script`: runs a JavaScript program in-process with the acus commands bound as functions,
//! and keeps saved scripts that run by name.

mod argv;
mod bindings;
mod cap;
mod editing;
mod host;
mod manage;
mod mcp;
mod memo;
mod net;
mod reads;
mod recommend;
mod result;
mod shapes;
mod store;
mod usage;

use crate::config::Config;
use acus_script::{HostError, Limits, ScriptError};
use anyhow::{Context, Result};
use cap::Cap;
use capsa_core::claude_settings::permissions::Refusal;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::hinted;
use capsa_core::render::{self, Doc, Render};
use serde::Serialize;
use serde_json::{Map, Value};
use std::io::Read;
use std::sync::atomic::Ordering;
use std::time::Instant;

/// Returns the script error for a call that the permission rules refuse.
fn refused(refusal: Refusal) -> HostError {
    let error = HostError::new(refusal.message);
    match refusal.hint {
        Some(hint) => error.with_hint(hint),
        None => error,
    }
}

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
    /// Output lines shown, prints then the return value; the return value keeps half. Default:
    /// `[script] output` in the config (400). The byte limit scales with it. `--json` is not capped.
    #[arg(long, value_name = "N")]
    max_output: Option<usize>,
    /// Parse the program and stop; nothing runs, no file or command changes.
    #[arg(long)]
    check: bool,
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
        None if a.check => check_inline(a.code, cfg, format),
        None => run_inline(a.code, a.max_output, cfg, format),
        Some(Saved::Save(s)) => render::emit(&manage::save(&s, cfg)?, format),
        Some(Saved::List) => render::emit(&manage::list(&cfg, &cwd()?), format),
        Some(Saved::Show(s)) => render::emit(&manage::show(&s, &cfg, &cwd()?)?, format),
        Some(Saved::Log(l)) => render::emit(&manage::history(&l, &cfg, &cwd()?)?, format),
        Some(Saved::Run(words)) => run_saved(&words, a.max_output, cfg, format),
    }
}

/// Reads the inline program from `-e` or stdin, and refuses an empty one.
fn read_program(code: Option<String>) -> Result<String> {
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
    Ok(code)
}

/// Parses the inline program and stops, so a typo costs no turn and changes nothing.
fn check_inline(code: Option<String>, cfg: Config, format: Format) -> Result<Outcome> {
    let code = read_program(code)?;
    let limits = limits(&cfg);
    acus_script::check(&code, bindings::Bindings::for_check(cfg)?, &limits).map_err(failure)?;
    let body = Checked { bytes: code.len() };
    render::emit(&body, format)
}

/// The result of `acus script --check`.
#[derive(Serialize)]
struct Checked {
    bytes: usize,
}

impl Render for Checked {
    fn doc(&self, doc: &mut Doc) {
        doc.title("acus script --check");
        doc.line(format!(
            "script ok: {} bytes; nothing ran, no file or command changed",
            self.bytes
        ));
    }
}

fn run_inline(
    code: Option<String>,
    max_output: Option<usize>,
    cfg: Config,
    format: Format,
) -> Result<Outcome> {
    let code = read_program(code)?;
    let limits = limits(&cfg);
    let cap = cap(&cfg, max_output);
    let log = usage::path(&cfg);
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let host = bindings::Bindings::new(cfg)?;
    let servers = host.mcp();
    let calls = host.calls();
    let started = Instant::now();
    let outcome = acus_script::run(&code, host, &limits);
    servers.shutdown();
    usage::record(
        &log,
        &usage::Inline {
            inline: true,
            cwd: logged_cwd(&cwd),
            ms: millis(started),
            ok: outcome.is_ok(),
            calls: calls.load(Ordering::Relaxed),
        },
    );
    finish(outcome, cap, format)
}

/// Runs `NAME[@V]` with the `key=value` words as `args`, and logs the run.
fn run_saved(
    words: &[String],
    max_output: Option<usize>,
    cfg: Config,
    format: Format,
) -> Result<Outcome> {
    let (spec, rest) = words.split_first().context("no script name given")?;
    let (name, version) = store::parse_spec(spec)?;
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let found = store::Store::open(&cfg, &cwd).locate(&name, version)?;
    let args = script_args(rest)?;
    let entry = format!("{}:{name}@{}", found.scope.as_str(), found.version);
    let log = usage::path(&cfg);
    let limits = limits(&cfg);
    let cap = cap(&cfg, max_output);
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
            cwd: logged_cwd(&cwd),
            ms: millis(started),
            ok: outcome.is_ok(),
        },
    );
    finish(outcome, cap, format)
}

/// Returns `cwd` resolved the way `recommend` resolves the directories it compares against, so
/// symlinks, Windows short names and verbatim prefixes cannot make the two differ.
fn logged_cwd(cwd: &std::path::Path) -> String {
    cwd.canonicalize()
        .unwrap_or_else(|_| cwd.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Returns the milliseconds since `started`, saturating at `u64::MAX`.
fn millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Returns the run's limits: the defaults, with the configured time limit.
fn limits(cfg: &Config) -> Limits {
    let mut limits = Limits::default();
    if let Some(secs) = cfg.script.timeout {
        limits.deadline = std::time::Duration::from_secs(secs);
    }
    limits
}

/// Returns the output cap: the configured one, or `--max-output` lines with the bytes scaled to
/// match.
fn cap(cfg: &Config, max_output: Option<usize>) -> Cap {
    let set = &cfg.script.output;
    match max_output {
        None => Cap {
            lines: set.lines,
            bytes: set.bytes,
        },
        Some(lines) => Cap {
            lines,
            bytes: set.bytes / set.lines.max(1) * lines.max(1),
        },
    }
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

/// Renders a finished run. A failed run renders what it printed and what completed unawaited
/// first, in the same form as a result, then returns its error for `exit` to print.
fn finish(
    outcome: Result<acus_script::Outcome, ScriptError>,
    cap: Cap,
    format: Format,
) -> Result<Outcome> {
    let error = match outcome {
        Ok(done) => {
            let shown = result::Output::new(done.prints, Vec::new(), done.value, cap, format);
            return render::emit(&shown, format);
        }
        Err(error) => error,
    };
    if let Some(partial) = error.partial() {
        let shown = result::Output::new(
            partial.prints.clone(),
            partial.completed.clone(),
            None,
            cap,
            format,
        );
        render::emit(&shown, format)?;
    }
    Err(failure(error))
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
