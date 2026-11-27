// SPDX-License-Identifier: MPL-2.0
//! `acus batch`: runs several acus commands from stdin, one per line, in parallel, and prints
//! each under its header in line order.

use crate::config::Config;
use crate::out::{clip, clip_lines, fit_line, fit_lines, write};
use crate::{Cli, Cmd};
use acus_guard::{BashDecision, Scope, Verdict, decide_bash};
use anyhow::{Context, Result};

use capsa_core::exit::Outcome;
use capsa_core::hint::{find_hint, hinted};
use clap::Parser;
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

mod lines;
use lines::{shown_args, split};

/// Characters `batch` prints before it cuts output, safely under Claude Code's 30k result cap.
const BUDGET: usize = 24_000;

/// Lines `batch` keeps from a line's output when the command takes `--head` and the line gives
/// no cap of its own. A line without `--head` (a test run, a patch) is left whole.
const DEFAULT_HEAD: usize = 100;

/// Longest failure reason `batch` repeats in its `failed:` footer.
const REASON: usize = 80;

/// Commands whose `--head N` shows the rest of their output.
const HEAD_COMMANDS: [&str; 7] = ["find", "outline", "diff", "files", "follow", "log", "map"];

#[derive(clap::Args)]
pub struct Args {
    /// Cap each line's output at N lines, unless that line sets --head itself. Defaults to 100
    /// for commands that take --head; a line without that flag is left whole.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// Clip each printed line of a command's output to N characters, unless that line sets
    /// --cut itself.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
}

/// Runs the acus commands on stdin, one per line, and prints each under a `== command` header.
/// Exit: 0 when a line found something, 1 when none did, 2 when a line failed.
pub fn run(a: &Args, cfg: &Config) -> Result<Outcome> {
    if std::io::stdin().is_terminal() {
        return Err(hinted(
            "acus batch reads its commands from stdin",
            "pipe them in with a heredoc: acus batch <<'EOF', one command per line, then EOF",
        ));
    }
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("cannot read stdin")?;
    let lines = plan(&input, cfg.script.strict);
    let exe = std::env::current_exe().context("cannot find the acus binary")?;
    let ran = par_map(&lines, |line| match &line.plan {
        Plan::Run { args, .. } => Some(spawn(&exe, args)),
        Plan::Refused { .. } => None,
    });
    print_all(a, &lines, ran)
}

/// One input line: its number, the text its header shows and what the batch does with it.
struct Line {
    number: usize,
    shown: String,
    plan: Plan,
}

/// What a line does: refused before it runs, or run as its own `acus` process.
enum Plan {
    Refused {
        reason: String,
        text: String,
    },
    /// `runs` is set for `run`, whose exit code 1 is the command's failure, not "nothing found".
    Run {
        runs: bool,
        args: Vec<String>,
    },
}

/// Splits the input into lines, dropping blank and `#` comment lines. A line that does not split
/// or that the checks refuse fails alone; the others still run.
fn plan(input: &str, strict: bool) -> Vec<Line> {
    let mut lines = Vec::new();
    for (i, raw) in input.lines().enumerate() {
        let number = i + 1;
        let args = match split(raw) {
            Ok(args) if args.is_empty() => continue,
            Ok(args) => args,
            Err(e) => {
                lines.push(Line {
                    number,
                    shown: raw.trim().to_owned(),
                    plan: refused(&e),
                });
                continue;
            }
        };
        let shown = shown_args(&args);
        let plan = match check(&args, strict) {
            Ok(runs) => Plan::Run { runs, args },
            Err(e) => refused(&e),
        };
        lines.push(Line {
            number,
            shown,
            plan,
        });
    }
    lines
}

/// Parses a line as the acus command it names and refuses the ones a batch cannot run. Returns
/// whether the command runs a shell command.
fn check(args: &[String], strict: bool) -> Result<bool> {
    let mut argv = vec!["acus".to_owned()];
    argv.extend(args.iter().cloned());
    let cli = Cli::try_parse_from(&argv).map_err(|e| crate::parse_error(&e, &argv))?;
    match &cli.cmd {
        Cmd::Patch(_) => Err(hinted(
            "`patch` cannot run in a batch",
            "run `acus patch` on its own, with its own heredoc",
        )),
        Cmd::Batch(_) => Err(hinted(
            "`batch` cannot be nested",
            "put the lines in one batch",
        )),
        #[cfg(feature = "script")]
        Cmd::Script(_) => Err(hinted(
            "`script` cannot run in a batch",
            "run `acus script` on its own",
        )),
        Cmd::Itself(_) => Err(hinted(
            "`acus self` cannot run in a batch",
            "run the command on its own",
        )),
        Cmd::Run(a) => {
            if let Some(command) = a.command() {
                guard(command, strict)?;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Refuses a shell command that a `Bash(...)` rule denies or asks about, or that the acus guard
/// would block, as `script`'s `run()` does.
fn guard(command: &str, strict: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    check_bash_rules(command, &cwd, strict)?;
    let scope = Scope::new(cwd.to_str());
    match decide_bash(command, scope.as_ref(), false) {
        BashDecision::Verdict(Verdict::Deny(_, reason)) => Err(hinted(
            "`run` refused the command: the acus guard blocks it",
            reason,
        )),
        _ => Ok(()),
    }
}

/// Applies the `Bash(...)` rules of the Claude Code settings, with the message `script` gives.
fn check_bash_rules(command: &str, cwd: &Path, strict: bool) -> Result<()> {
    crate::permission_mode::permissions(cwd.to_path_buf(), strict)
        .check_bash(command)
        .map_err(|e| match e.hint {
            Some(hint) => hinted(e.message, hint),
            None => anyhow::anyhow!(e.message),
        })
}

/// The plan of a line refused with `e`: its printed text and the short reason its footer repeats.
fn refused(e: &anyhow::Error) -> Plan {
    let msg = format!("{e:#}");
    let text = match find_hint(e) {
        Some(hint) => format!("error: {msg}\nhint: {hint}\n"),
        None => format!("error: {msg}\n"),
    };
    Plan::Refused {
        reason: short(&msg),
        text,
    }
}

/// A failure reason for the footer: its first line, capped at `REASON` chars.
fn short(msg: &str) -> String {
    clip(msg.lines().next().unwrap_or_default(), 0, REASON)
}

/// Maps `f` over `items` on all cores and returns the results in item order. Workers take the
/// next item from a shared counter, so one slow command does not hold the others back.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut done = Vec::with_capacity(items.len());
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        mine.push((i, f(item)));
                    }
                    mine
                })
            })
            .collect();
        for h in handles {
            done.extend(h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)));
        }
    });
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, r)| r).collect()
}

/// Runs one line as its own `acus` process. Its stdin is empty, so `find` does not search a pipe.
fn spawn(exe: &Path, args: &[String]) -> std::io::Result<Output> {
    Command::new(exe).args(args).stdin(Stdio::null()).output()
}

/// Prints each line under its header in line order, cutting each command's output to its share
/// of the budget, then the footer naming the failed lines.
fn print_all(
    a: &Args,
    lines: &[Line],
    ran: Vec<Option<std::io::Result<Output>>>,
) -> Result<Outcome> {
    let total = lines.len();
    let (mut budget, mut found, mut failed) = (BUDGET, false, Vec::new());
    for (k, (line, ran)) in lines.iter().zip(ran).enumerate() {
        let header = format!("== {}\n", line.shown);
        budget = budget.saturating_sub(header.chars().count());
        write(&header);
        let (body, step) = settle(line, ran);
        let share = budget.checked_div(total - k).unwrap_or(usize::MAX);
        let body = match &line.plan {
            Plan::Run { args, .. } => {
                let body = cap_lines(a, args, body);
                if body.chars().count() > share {
                    budget_cut(&body, share, args)
                } else {
                    body
                }
            }
            Plan::Refused { .. } => body,
        };
        budget = budget.saturating_sub(body.chars().count());
        write(&body);
        match step {
            Ok(Step::Found) => found = true,
            Ok(Step::Empty) => {}
            Err(reason) => failed.push((line.number, reason)),
        }
    }
    if !failed.is_empty() {
        let list: Vec<String> = failed
            .iter()
            .map(|(n, r)| format!("line {n}: {r}"))
            .collect();
        write(&format!(
            "failed: {} of {total} lines ({})\n",
            failed.len(),
            list.join(", ")
        ));
        anyhow::bail!("{} of {total} lines failed", failed.len());
    }
    Ok(if found {
        Outcome::Found
    } else {
        Outcome::Empty
    })
}

/// How a line that ran came out: something was found, or nothing was.
#[derive(PartialEq, Eq)]
enum Step {
    Found,
    Empty,
}

/// The text a line prints and its step, or the reason it failed.
fn settle(line: &Line, ran: Option<std::io::Result<Output>>) -> (String, Result<Step, String>) {
    let (runs, out) = match (&line.plan, ran) {
        (Plan::Refused { reason, text }, _) => return (text.clone(), Err(reason.clone())),
        (Plan::Run { runs, .. }, Some(Ok(out))) => (*runs, out),
        (Plan::Run { .. }, other) => {
            let msg = other
                .and_then(Result::err)
                .map_or_else(|| "not run".to_owned(), |e| e.to_string());
            return (format!("error: cannot run acus: {msg}\n"), Err(short(&msg)));
        }
    };
    let step = judge(runs, &out);
    let mut text = combined(&out);
    if step == Ok(Step::Empty) && text.is_empty() {
        text = "(no results)\n".to_owned();
    }
    (text, step)
}

/// Reads a finished line's exit code. For `run` the code is the command's own, so 1 is a failure;
/// for the other commands 1 means nothing found, and anything else is a failure named by its
/// `error:` line.
fn judge(runs: bool, out: &Output) -> Result<Step, String> {
    match out.status.code() {
        Some(0) => Ok(Step::Found),
        Some(1) if !runs => Ok(Step::Empty),
        Some(code) => Err(short(
            &error_line(&out.stderr).unwrap_or_else(|| format!("exit code {code}")),
        )),
        None => Err("stopped by a signal".to_owned()),
    }
}

/// The first `error:` line of a command's stderr, without the prefix.
fn error_line(stderr: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .find_map(|line| line.strip_prefix("error: "))
        .map(str::to_owned)
}

/// A line's stdout followed by its stderr, ending in a newline when it has any text.
fn combined(out: &Output) -> String {
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Cuts `body` after the last whole line that fits in `share` chars and names the command that
/// shows the rest. A first line that does not fit is clipped instead of dropped.
fn budget_cut(body: &str, share: usize, args: &[String]) -> String {
    let (kept, dropped) = fit_lines(body, share);
    let next = rerun(args, kept.lines().count() + dropped);
    if kept.is_empty() {
        let first = fit_line(body.lines().next().unwrap_or_default(), share);
        let chars = first.chars().count() + 1;
        return format!("{first}\n… cut at {chars} chars: {next}\n");
    }
    let chars = kept.chars().count();
    let mut out = kept.to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("… cut at {chars} chars: {next}\n"));
    out
}

/// Applies the batch's `--cut`, then its `--head`, to a line's output unless the line sets the flag.
fn cap_lines(a: &Args, args: &[String], body: String) -> String {
    let body = match a.cut {
        Some(n) if !has_flag(args, "--cut") => clip_lines(&body, as_usize(n)),
        _ => body,
    };
    let head = a
        .head
        .map(as_usize)
        .or_else(|| takes_head(args).then_some(DEFAULT_HEAD));
    match head {
        Some(n) if !has_flag(args, "--head") && !has_flag(args, "--full") && !json_output(args) => {
            head_text(&body, n, args)
        }
        _ => body,
    }
}

/// Whether the line asks for JSON, which must not be cut by line.
fn json_output(args: &[String]) -> bool {
    has_flag(args, "--json")
        || args.iter().any(|a| a == "--format=json")
        || args
            .windows(2)
            .any(|w| w[0] == "--format" && w[1] == "json")
}

/// Whether the line's command accepts `--head` or can lift a cut with `--full`.
fn takes_head(args: &[String]) -> bool {
    let cmd = args.first().map_or("", String::as_str);
    HEAD_COMMANDS.contains(&cmd)
        || cmd == "show"
        || (cmd == "diff" && args.iter().any(|a| a == "-p"))
}

/// Keeps the first `n` lines of `body` and names the command that shows the rest.
fn head_text(body: &str, n: usize, args: &[String]) -> String {
    let count = body.lines().count();
    if count <= n {
        return body.to_owned();
    }
    let mut kept: String = body.split_inclusive('\n').take(n).collect();
    if !kept.ends_with('\n') {
        kept.push('\n');
    }
    let more = count - n;
    kept.push_str(&format!(
        "… output cut at --head {n}; {more} more lines: {}\n",
        rerun(args, count)
    ));
    kept
}

/// Whether the line already sets `flag`, as `--head N` or `--head=N`.
fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.split_once('=').is_some_and(|(name, _)| name == flag))
}

fn as_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// The command that shows the rest of a cut line: `--full` for `show` and `diff -p`, `--head N`
/// for the commands that take it, and otherwise the line alone, which is not cut.
fn rerun(args: &[String], lines: usize) -> String {
    let cmd = args.first().map_or("", String::as_str);
    let full = cmd == "show" || (cmd == "diff" && args.iter().any(|a| a == "-p"));
    let rerun = if full {
        with_flag(args, "--full", None)
    } else if HEAD_COMMANDS.contains(&cmd) {
        with_flag(args, "--head", Some(lines))
    } else {
        args.to_vec()
    };
    format!("acus {}", shown_args(&rerun))
}

/// `args` with `flag` set to `value`, replacing an existing `--flag value` or `--flag=value`.
fn with_flag(args: &[String], flag: &str, value: Option<usize>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len() + 2);
    let (mut i, mut set) = (0, false);
    while i < args.len() {
        let a = &args[i];
        if a == flag {
            set = true;
            i += 1;
            if value.is_some() {
                i += 1;
            }
            out.push(flag.to_owned());
            if let Some(v) = value {
                out.push(v.to_string());
            }
            continue;
        }
        if let Some(v) = value
            && let Some((name, _)) = a.split_once('=')
            && name == flag
        {
            out.push(format!("{flag}={v}"));
            set = true;
            i += 1;
            continue;
        }
        out.push(a.clone());
        i += 1;
    }
    if !set {
        out.push(flag.to_owned());
        if let Some(v) = value {
            out.push(v.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_a_head_capable_line_at_the_default() {
        let args = ["find", "x"].map(str::to_owned);
        let body: String = (0..DEFAULT_HEAD + 5)
            .map(|i| format!("line {i}\n"))
            .collect();
        let capped = cap_lines(
            &Args {
                head: None,
                cut: None,
            },
            &args,
            body,
        );
        assert!(capped.contains("output cut at --head 100"), "{capped}");
        assert!(capped.contains("acus find x --head 105"), "{capped}");
    }

    #[test]
    fn leaves_a_full_show_whole_without_an_explicit_head() {
        let args = ["show", "big.rs", "--full"].map(str::to_owned);
        let body: String = (0..DEFAULT_HEAD + 5)
            .map(|i| format!("line {i}\n"))
            .collect();
        assert_eq!(
            cap_lines(
                &Args {
                    head: None,
                    cut: None
                },
                &args,
                body.clone()
            ),
            body
        );
    }

    #[test]
    fn leaves_a_json_line_whole_without_an_explicit_head() {
        for args in [
            vec!["show", "big.rs", "--json"],
            vec!["show", "big.rs", "--format", "json"],
            vec!["show", "big.rs", "--format=json"],
        ] {
            let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
            let body: String = (0..DEFAULT_HEAD + 5)
                .map(|i| format!("line {i}\n"))
                .collect();
            assert_eq!(
                cap_lines(
                    &Args {
                        head: None,
                        cut: None
                    },
                    &args,
                    body.clone()
                ),
                body
            );
        }
    }

    #[test]
    fn leaves_a_run_line_whole_without_an_explicit_head() {
        let args = ["run", "cargo test"].map(str::to_owned);
        let body: String = (0..DEFAULT_HEAD + 5)
            .map(|i| format!("line {i}\n"))
            .collect();
        assert_eq!(
            cap_lines(
                &Args {
                    head: None,
                    cut: None
                },
                &args,
                body.clone()
            ),
            body
        );
    }

    #[test]
    fn replaces_an_existing_head_instead_of_appending() {
        let args = ["find", "x", "--head", "5"].map(str::to_owned);
        assert_eq!(
            with_flag(&args, "--head", Some(9)),
            ["find", "x", "--head", "9"]
        );
        let args = ["find", "x", "--head=5"].map(str::to_owned);
        assert_eq!(
            with_flag(&args, "--head", Some(9)),
            ["find", "x", "--head=9"]
        );
        let args = ["find", "x"].map(str::to_owned);
        assert_eq!(
            with_flag(&args, "--head", Some(9)),
            ["find", "x", "--head", "9"]
        );
    }

    #[test]
    fn names_the_rerun_that_lifts_each_cap() {
        let find = ["find", "x", "--head", "5"].map(str::to_owned);
        assert_eq!(rerun(&find, 12), "acus find x --head 12");
        let show = ["show", "big.rs"].map(str::to_owned);
        assert_eq!(rerun(&show, 60), "acus show big.rs --full");
        let diff = ["diff", "-p", "HEAD"].map(str::to_owned);
        assert_eq!(rerun(&diff, 80), "acus diff -p HEAD --full");
        let blame = ["blame", "a.rs#f"].map(str::to_owned);
        assert_eq!(rerun(&blame, 5), "acus blame a.rs#f");
    }

    #[test]
    fn clips_a_first_line_too_long_for_its_share() {
        let body = format!("{}\nsecond\n", "x".repeat(30_000));
        let cut = budget_cut(&body, 100, &["show".to_owned(), "min.js".to_owned()]);
        let first = cut.lines().next().unwrap_or_default();
        assert!(first.chars().count() < 100, "{}", first.chars().count());
        assert!(first.contains("(+"), "{first}");
        assert!(cut.ends_with("chars: acus show min.js --full\n"), "{cut}");
    }
}
