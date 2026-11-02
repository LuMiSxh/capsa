// SPDX-License-Identifier: MPL-2.0
//! `acus run`: runs several acus commands from stdin, one per line or as a JSON array.

use crate::fmt::Out;
use crate::{Cli, execute};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use clap::Parser;
use std::io::{Read, Write};

#[derive(clap::Args)]
pub struct Args {}

/// Reads one command per line (`find x --block`, shell-style quotes, `#` comments) or
/// `[["find","x"],["show","a.rs#f"]]` from stdin and runs each entry in order.
/// Agent/human output separates entries with `>>> acus …` lines; JSON output is one
/// line per entry. Exit: 2 if any entry failed, 0 if any succeeded, otherwise 1.
pub fn run(_: Args, out: &Out) -> Result<Outcome> {
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
    let (mut found, mut failed) = (false, false);
    for args in batch {
        let args = match args {
            Ok(a) => a,
            Err(e) => {
                failed = true;
                match out.format {
                    Format::Json => {
                        println!("{}", serde_json::json!({ "error": format!("{e:#}") }));
                    }
                    _ => println!("error: {e:#}"),
                }
                continue;
            }
        };
        let mut argv = vec!["acus".to_owned()];
        argv.extend(args.iter().cloned());
        if out.format == Format::Json {
            argv.push("--json".into());
        } else {
            // Quote arguments with spaces so the echo can be pasted back.
            let shown: Vec<_> = args
                .iter()
                .map(|a| {
                    if a.is_empty() || a.contains(char::is_whitespace) {
                        format!("\"{a}\"")
                    } else {
                        a.clone()
                    }
                })
                .collect();
            println!("{}", out.header(&format!(">>> acus {}", shown.join(" "))));
        }
        let res = Cli::try_parse_from(&argv)
            .map_err(|e| crate::parse_error(&e, &argv))
            .and_then(|cli| {
                let format = capsa_core::cli::App::format(&cli);
                execute(cli, format, true)
            });
        std::io::stdout().flush().ok();
        match res {
            // Code(0) is a passing `ctx`, or a diff with no changes, which prints its own `null`.
            Ok(Outcome::Found | Outcome::Code(0)) => found = true,
            // A `ctx` command that failed counts as a failed entry.
            Ok(Outcome::Code(_)) => failed = true,
            // JSON output keeps one line per entry, so an empty result is `null`.
            Ok(Outcome::Empty) => match out.format {
                Format::Json => println!("null"),
                _ => println!("(no results)"),
            },
            Err(e) => {
                failed = true;
                let msg = entry_error(&e);
                if out.format == Format::Json {
                    println!("{}", serde_json::json!({ "error": msg }));
                } else {
                    println!("error: {msg}");
                }
            }
        }
    }
    if failed {
        anyhow::bail!("some commands failed (see above)");
    }
    Ok(if found {
        Outcome::Found
    } else {
        Outcome::Empty
    })
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

#[cfg(test)]
mod tests {
    #[test]
    fn splits_command_lines() {
        let s = |l| super::split(l).unwrap();
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
        assert!(super::split("find 'x").is_err());
    }
}
