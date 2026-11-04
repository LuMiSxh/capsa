// SPDX-License-Identifier: MPL-2.0
//! `acus run`: runs several acus commands from stdin, one per line or as a JSON array.

use crate::fmt::{Out, capture, clip};
use crate::{Cli, execute};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use clap::Parser;
use std::io::{Read, Write};

/// Characters `run` prints before it cuts lines, safely under Claude Code's 30k result cap.
const BUDGET: usize = 24_000;

/// Longest failure reason `run` repeats in its `failed:` footer.
const REASON: usize = 80;

/// Commands whose `--head N` shows the rest of their output.
const HEAD_COMMANDS: [&str; 4] = ["find", "show", "outline", "diff"];

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
#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .context("cannot read stdin")?;
    // A line that does not split (unclosed quote) fails alone instead of the whole batch.
    let batch: Vec<Result<Vec<String>>> = if s.trim_start().starts_with('[') {
        let v: Vec<Vec<String>> = serde_json::from_str(&s).map_err(|e| {
            capsa_core::hint::Hinted::new(
                format!("expected a JSON array of argument arrays: {e}"),
                r#"[["find","needle"],["show","src/a.rs#f"]]"#,
            )
        })?;
        v.into_iter().map(Ok).collect()
    } else {
        s.lines()
            .map(split)
            .filter(|a| !matches!(a, Ok(a) if a.is_empty()))
            .collect()
    };
    let total = batch.len();
    let text = out.format != Format::Json;
    let mut budget = if a.full || !text { usize::MAX } else { BUDGET };
    let (mut found, mut failed) = (false, Vec::new());
    for (i, args) in batch.into_iter().enumerate() {
        let line = i + 1;
        let args = match args {
            Ok(args) => args,
            Err(e) => {
                let msg = format!("{e:#}");
                failed.push((line, short(&msg)));
                entry_error_line(out, &msg);
                if a.fail_fast {
                    break;
                }
                continue;
            }
        };
        let mut argv = vec!["acus".to_owned()];
        argv.extend(args.iter().cloned());
        if !text {
            argv.push("--json".into());
        }
        let shown = shown_args(&args);
        if text {
            let header = out.header(&format!(">>> acus {shown}"));
            budget = budget.saturating_sub(header.chars().count() + 1);
            outln!("{header}");
        }
        let (res, mut body) = capture(|| {
            Cli::try_parse_from(&argv)
                .map_err(|e| crate::parse_error(&e, &argv))
                .and_then(|cli| {
                    let format = capsa_core::cli::App::format(&cli);
                    execute(cli, format, true)
                })
        });
        std::io::stdout().flush().ok();
        if text {
            if let (Some(n), false) = (a.cut, has_flag(&args, "--cut")) {
                body = cut_text(&body, as_usize(n));
            }
            if let (Some(n), false) = (a.head, has_flag(&args, "--head")) {
                body = head_text(&body, as_usize(n));
            }
        }
        let mut status = Ok(());
        match res {
            // Code(0) is a passing `ctx`, or a diff with no changes, which prints its own `null`.
            Ok(Outcome::Found | Outcome::Code(0)) => found = true,
            Ok(Outcome::Code(code)) => status = Err(format!("exit code {code}")),
            // JSON output keeps one line per entry, so an empty result is `null`.
            Ok(Outcome::Empty) => body.push_str(if text { "(no results)\n" } else { "null\n" }),
            Err(e) => status = Err(entry_error(&e)),
        }
        if text && !body.is_empty() {
            let share = budget.checked_div(total - i).unwrap_or(usize::MAX);
            if body.chars().count() > share {
                body = budget_cut(&body, share, &args, &shown);
            }
            budget = budget.saturating_sub(body.chars().count());
        }
        crate::fmt::out(&body);
        if let Err(msg) = status {
            failed.push((line, short(&msg)));
            entry_error_line(out, &msg);
            if a.fail_fast {
                break;
            }
        }
    }
    failure_footer(out, &failed, total);
    if a.fail_fast && !failed.is_empty() {
        anyhow::bail!("--fail-fast stopped at line {}", failed[0].0);
    }
    if !failed.is_empty() && failed.len() == total {
        anyhow::bail!("all {total} lines failed");
    }
    Ok(if found {
        Outcome::Found
    } else {
        Outcome::Empty
    })
}

fn as_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// Whether the line already sets `flag`, as `--head N` or `--head=N`.
fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{flag}=")))
}

/// The line's command with quotes around arguments that need them, so it can be pasted back.
fn shown_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.contains(char::is_whitespace) {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
/// shows the rest.
fn budget_cut(body: &str, share: usize, args: &[String], shown: &str) -> String {
    let (kept, dropped) = fit_lines(body, share);
    let chars = kept.chars().count();
    let total = kept.lines().count() + dropped;
    let more = if args
        .first()
        .is_some_and(|c| HEAD_COMMANDS.contains(&c.as_str()))
    {
        format!("{shown} --head {total}")
    } else {
        "acus run --full".to_owned()
    };
    let mut out = kept.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("… cut at {chars} chars: {more}\n"));
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
}
