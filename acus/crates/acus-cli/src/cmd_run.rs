// SPDX-License-Identifier: MPL-2.0
//! `acus run`: runs several acus commands from stdin, one per line or as a JSON array.

use crate::out::{Out, capture, clip, fit_line};
use crate::{Cli, execute};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::hinted;
use clap::Parser;
use std::io::{Read, Write};

/// Characters `run` prints before it cuts lines, safely under Claude Code's 30k result cap.
const BUDGET: usize = 24_000;

/// Longest failure reason `run` repeats in its `failed:` footer.
const REASON: usize = 80;

/// Commands whose `--head N` shows the rest of their output.
const HEAD_COMMANDS: [&str; 4] = ["find", "show", "outline", "diff"];

/// `self` commands write through `render::emit` and `update`, outside the capture sink.
const SELF_REFUSED: &str =
    "`acus self` cannot run inside `acus run`\nhint: run the command on its own";

#[derive(clap::Args)]
pub struct Args {
    /// Stop at the first failing line and exit 2.
    #[arg(long)]
    fail_fast: bool,
    /// Cap every line's output at N lines, unless that line sets --head itself.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// Clip every line's output to N characters, unless that line sets --cut itself.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
    /// Lift the total output budget and print every line in full.
    #[arg(long)]
    full: bool,
}

/// Reads one command per line (`find x --block`, shell-style quotes, `#` comments) or
/// `[["find","x"],["show","a.rs#f"]]` from stdin and runs each entry in order.
/// Agent/human output separates entries with `>>> acus …` lines; JSON output is one line per
/// entry. Exit: 0 when a line succeeded, 2 when every line failed or `--fail-fast` stopped at
/// one, otherwise 1. A footer lists the failed lines unless all of them failed.
pub fn run(a: &Args, out: &Out) -> Result<Outcome> {
    let batch = read_batch()?;
    let text = out.format != Format::Json;
    let mut state = Batch {
        a,
        out,
        text,
        total: batch.len(),
        budget: if a.full || !text { usize::MAX } else { BUDGET },
        found: false,
        failed: Vec::new(),
        stop: false,
    };
    for (i, args) in batch.into_iter().enumerate() {
        state.entry(i + 1, args);
        if state.stop {
            break;
        }
    }
    state.finish()
}

/// The state of one `run` call: the shared budget and the lines that ran.
struct Batch<'a> {
    a: &'a Args,
    out: &'a Out,
    text: bool,
    total: usize,
    budget: usize,
    found: bool,
    failed: Vec<(usize, String)>,
    stop: bool,
}

impl Batch<'_> {
    /// Runs one line, records its failure and stops the batch after it under `--fail-fast`.
    fn entry(&mut self, number: usize, args: Result<Vec<String>>) {
        let args = match args {
            Ok(args) => args,
            Err(e) => return self.fail(number, &format!("{e:#}")),
        };
        if self.text {
            let header = self.out.header(&format!(">>> acus {}", shown_args(&args)));
            self.budget = self.budget.saturating_sub(header.chars().count() + 1);
            outln!("{header}");
        }
        if args.first().is_some_and(|c| c == "self") {
            return self.fail(number, SELF_REFUSED);
        }
        let (status, body) = self.call(&args);
        std::io::stdout().flush().ok();
        let body = if self.text {
            self.cap(&args, number - 1, body)
        } else {
            body
        };
        crate::out::write(&body);
        if let Err(msg) = status {
            self.fail(number, &msg);
        }
    }

    /// Runs the line in the capture sink; returns its failure reason and printed output.
    fn call(&mut self, args: &[String]) -> (Result<(), String>, String) {
        let mut argv = vec!["acus".to_owned()];
        argv.extend(args.iter().cloned());
        if !self.text {
            argv.push("--json".into());
        }
        let (res, body) = capture(|| {
            Cli::try_parse_from(&argv)
                .map_err(|e| crate::parse_error(&e, &argv))
                .and_then(|cli| {
                    let format = capsa_core::cli::App::format(&cli);
                    execute(cli, format, true)
                })
        });
        match res {
            // Code(0) is a passing `ctx`, or a diff with no changes, which prints its own `null`.
            Ok(Outcome::Found | Outcome::Code(0)) => {
                self.found = true;
                (Ok(()), body)
            }
            Ok(Outcome::Code(code)) => (Err(format!("exit code {code}")), body),
            // JSON output keeps one line per entry, so an empty result is `null`.
            Ok(Outcome::Empty) => {
                let empty = if self.text {
                    "(no results)\n"
                } else {
                    "null\n"
                };
                (Ok(()), body + empty)
            }
            Err(e) => (Err(entry_error(&e)), body),
        }
    }

    /// Applies the line's `--cut`/`--head`, then the shared character budget, to `body`.
    fn cap(&mut self, args: &[String], index: usize, mut body: String) -> String {
        if let (Some(n), false) = (self.a.cut, has_flag(args, "--cut")) {
            body = cut_text(&body, as_usize(n));
        }
        if let (Some(n), false) = (self.a.head, has_flag(args, "--head")) {
            body = head_text(&body, as_usize(n));
        }
        if body.is_empty() {
            return body;
        }
        let share = self
            .budget
            .checked_div(self.total - index)
            .unwrap_or(usize::MAX);
        if body.chars().count() > share {
            body = budget_cut(&body, share, args);
        }
        self.budget = self.budget.saturating_sub(body.chars().count());
        body
    }

    /// Records a failed line, prints its error and stops after it under `--fail-fast`.
    fn fail(&mut self, number: usize, msg: &str) {
        self.failed.push((number, short(msg)));
        entry_error_line(self.out, msg);
        self.stop = self.a.fail_fast;
    }

    fn finish(self) -> Result<Outcome> {
        failure_footer(self.out, &self.failed, self.total);
        if self.a.fail_fast && !self.failed.is_empty() {
            anyhow::bail!("--fail-fast stopped at line {}", self.failed[0].0);
        }
        if !self.failed.is_empty() && self.failed.len() == self.total {
            anyhow::bail!("all {} lines failed", self.total);
        }
        Ok(if self.found {
            Outcome::Found
        } else {
            Outcome::Empty
        })
    }
}

/// Reads one command per line or a JSON array of argument arrays from stdin.
fn read_batch() -> Result<Vec<Result<Vec<String>>>> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .context("cannot read stdin")?;
    if s.trim_start().starts_with('[') {
        let v: Vec<Vec<String>> = serde_json::from_str(&s).map_err(|e| {
            hinted(
                format!("expected a JSON array of argument arrays: {e}"),
                r#"[["find","needle"],["show","src/a.rs#f"]]"#,
            )
        })?;
        return Ok(v.into_iter().map(Ok).collect());
    }
    // A line that does not split (unclosed quote) fails alone instead of the whole batch.
    Ok(s.lines()
        .map(split)
        .filter(|a| !matches!(a, Ok(a) if a.is_empty()))
        .collect())
}

fn as_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// Whether the line already sets `flag`, as `--head N` or `--head=N`.
fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{flag}=")))
}

/// The line's command with shell quoting, so it can be pasted back.
fn shown_args(args: &[String]) -> String {
    args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

/// Quotes `a` in single quotes unless every character is one a shell leaves alone.
fn quote(a: &str) -> String {
    let safe = !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:#=@%+,-".contains(c));
    if safe {
        a.to_owned()
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

/// Keeps the first N lines and names the rerun that shows the rest.
fn head_text(body: &str, n: usize) -> String {
    let count = body.lines().count();
    if count <= n {
        return body.to_owned();
    }
    let mut kept = body.lines().take(n).collect::<Vec<_>>().join("\n");
    kept.push('\n');
    kept.push_str(&format!(
        "… output cut at --head {n}; {} more lines (rerun with --head {count})",
        count - n
    ));
    kept.push('\n');
    kept
}

/// Clips each line as `--cut N` does, keeping the line breaks.
fn cut_text(body: &str, n: usize) -> String {
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        if let Some(text) = line.strip_suffix('\n') {
            out.push_str(&clip(text, 0, n));
            out.push('\n');
        } else {
            out.push_str(&clip(line, 0, n));
        }
    }
    out
}

/// Cuts `body` after the last whole line that fits in `share` chars and names the command that
/// shows the rest. A first line that does not fit is clipped instead of dropped.
fn budget_cut(body: &str, share: usize, args: &[String]) -> String {
    let (kept, dropped) = fit_lines(body, share);
    if kept.is_empty() && !body.is_empty() {
        return first_line_cut(body, share);
    }
    let chars = kept.chars().count();
    let lines = kept.lines().count() + dropped;
    let mut out = kept.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("… cut at {chars} chars: {}\n", rerun(args, lines)));
    out
}

/// Keeps the first line clipped to `share` chars and names the flag that lifts the budget.
fn first_line_cut(body: &str, share: usize) -> String {
    let first = body.lines().next().unwrap_or_default();
    let clipped = fit_line(first, share);
    let chars = clipped.chars().count() + 1;
    format!("{clipped}\n… cut at {chars} chars: acus run --full\n")
}

/// The command that shows the rest of a cut entry, replacing an existing `--head` instead of
/// repeating it. `--head` cannot lift the whole-file cap of `show` or the per-file cap of
/// `diff -p`, so those name `--full`.
fn rerun(args: &[String], lines: usize) -> String {
    let cmd = args.first().map(String::as_str).unwrap_or_default();
    let full = cmd == "show" || (cmd == "diff" && args.iter().any(|a| a == "-p" || a == "--lines"));
    let rerun = if full {
        with_flag(args, "--full", None)
    } else if HEAD_COMMANDS.contains(&cmd) {
        with_flag(args, "--head", Some(lines))
    } else {
        return "acus run --full".to_owned();
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
        if let (Some(v), Some((name, _))) = (value, a.split_once('='))
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

/// The longest prefix of whole lines at most `max` chars long, and how many lines it leaves out.
fn fit_lines(body: &str, max: usize) -> (&str, usize) {
    let (mut used, mut end, mut dropped, mut fits) = (0, 0, 0, true);
    for line in body.split_inclusive('\n') {
        let n = line.chars().count();
        if fits && used + n <= max {
            used += n;
            end += line.len();
        } else {
            fits = false;
            dropped += 1;
        }
    }
    (body.get(..end).unwrap_or_default(), dropped)
}

/// The footer listing the failed lines; `--json` keeps one line per entry and no footer.
fn failure_footer(out: &Out, failed: &[(usize, String)], total: usize) {
    if failed.is_empty() || out.format == Format::Json {
        return;
    }
    let lines: Vec<_> = failed
        .iter()
        .map(|(line, error)| format!("line {line}: {error}"))
        .collect();
    outln!(
        "failed: {} of {total} lines ({})",
        failed.len(),
        lines.join(", ")
    );
}

/// A failure reason for the footer: its first line, capped at `REASON` chars.
fn short(msg: &str) -> String {
    clip(msg.lines().next().unwrap_or_default(), 0, REASON)
}

/// Splits a command line like a shell would for plain words and '…'/"…" quotes; drops a
/// leading `acus` and `#` comments. Backslashes are literal outside double quotes (Windows
/// paths).
fn split(line: &str) -> Result<Vec<String>> {
    let (mut args, mut cur, mut word) = (Vec::new(), String::new(), false);
    let mut chars = line.trim().chars();
    while let Some(c) = chars.next() {
        match c {
            '#' if !word => break,
            c if c.is_whitespace() => {
                if word {
                    args.push(std::mem::take(&mut cur));
                    word = false;
                }
            }
            '\'' | '"' => {
                word = true;
                loop {
                    match chars.next() {
                        Some(q) if q == c => break,
                        Some('\\') if c == '"' => cur.extend(chars.next()),
                        Some(x) => cur.push(x),
                        None => anyhow::bail!("unclosed {c} in `{line}`"),
                    }
                }
            }
            c => {
                word = true;
                cur.push(c);
            }
        }
    }
    if word {
        args.push(cur);
    }
    if args.first().is_some_and(|a| a == "acus") {
        args.remove(0);
    }
    Ok(args)
}

/// The text of a failed entry: the error chain, then its `hint:` line when it has one.
fn entry_error(e: &anyhow::Error) -> String {
    let mut msg = format!("{e:#}");
    if let Some(hint) = capsa_core::hint::find_hint(e) {
        msg.push_str("\nhint: ");
        msg.push_str(hint);
    }
    msg
}

/// Prints a failed entry as an `error:` line, or as a JSON object under `--json`.
fn entry_error_line(out: &Out, msg: &str) {
    if out.format == Format::Json {
        outln!("{}", serde_json::json!({ "error": msg }));
    } else {
        outln!("error: {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_command_lines() {
        let s = |l| split(l).unwrap();
        assert_eq!(
            s("acus find 'fn (a|b)' --block # why"),
            ["find", "fn (a|b)", "--block"]
        );
        assert_eq!(
            s(r#"show "a b.rs#f" src\x.rs"#),
            ["show", "a b.rs#f", r"src\x.rs"]
        );
        assert_eq!(s("show a.rs#f"), ["show", "a.rs#f"]);
        assert!(s("  # only a comment").is_empty());
        assert!(split("find 'x").is_err());
    }

    #[test]
    fn fits_whole_lines_and_counts_the_rest() {
        let (kept, dropped) = fit_lines("aaa\nbb\nc\n", 8);
        assert_eq!((kept, dropped), ("aaa\nbb\n", 1));
        let (kept, dropped) = fit_lines("long line\nshort\n", 4);
        assert_eq!((kept, dropped), ("", 2));
    }

    #[test]
    fn head_keeps_lines_and_names_the_rerun() {
        let cut = head_text("a\nb\nc\n", 2);
        assert_eq!(
            cut,
            "a\nb\n… output cut at --head 2; 1 more lines (rerun with --head 3)\n"
        );
    }

    #[test]
    fn quotes_arguments_a_shell_would_split() {
        let args = ["find", "foo|bar", "src", "-g", "*.rs", "$x", "a b", "it's"].map(str::to_owned);
        assert_eq!(
            shown_args(&args),
            r#"find 'foo|bar' src -g '*.rs' '$x' 'a b' 'it'\''s'"#
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
        let log = ["log"].map(str::to_owned);
        assert_eq!(rerun(&log, 5), "acus run --full");
    }

    #[test]
    fn clips_a_first_line_too_long_for_its_share() {
        let body = format!("{}\nsecond\n", "x".repeat(30_000));
        let cut = budget_cut(&body, 100, &["show".to_owned(), "min.js".to_owned()]);
        let first = cut.lines().next().unwrap_or_default();
        assert!(first.chars().count() < 100, "{}", first.chars().count());
        assert!(first.contains("(+"), "{first}");
        assert!(cut.ends_with("chars: acus run --full\n"), "{cut}");
        assert!(!cut.contains("--head"), "{cut}");
    }
}
