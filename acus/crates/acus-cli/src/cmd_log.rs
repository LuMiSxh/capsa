// SPDX-License-Identifier: MPL-2.0
//! `acus log`: a compact git log, with the changed lines of each commit.

use crate::git::git;
use crate::out::{Budget, Out, clip};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::render::{Align, Doc, Table, Tone, paint};
use std::io;
use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    /// Optional revision (default HEAD, or `A..B`), then paths to limit to.
    #[arg(value_parser = crate::home::<String>)]
    args: Vec<String>,
    /// Number of commits.
    #[arg(short = 'n', long, default_value_t = 15)]
    max_count: usize,
    /// Stop after N printed lines, headers and footers included; the rest is counted in a footer.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// Clip each printed line to N characters; a clipped line ends with `… (+N chars)`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
    /// Accepted for git habits: one line per commit is the only format.
    #[arg(long = "oneline", hide = true)]
    _oneline: bool,
}

struct Commit {
    hash: String,
    at: i64,
    date: String,
    author: String,
    subject: String,
    files: usize,
    add: usize,
    del: usize,
}

/// Compact `git log`: one line per commit with date, author and changed lines.
pub fn run(a: &Args, out: &Out) -> Result<Outcome> {
    // Like git: the first argument is a revision unless it names an existing path.
    let (rev, paths) = match a.args.split_first() {
        Some((r, rest)) if !Path::new(r).exists() => (Some(r.as_str()), rest),
        _ => (None, &a.args[..]),
    };
    let max = format!("--max-count={}", a.max_count);
    let mut cmd = vec![
        "-c",
        "core.quotepath=off",
        "log",
        "--no-color",
        "--no-ext-diff",
        "--numstat",
        "--date=short",
        "--format=%x01%h%x02%at%x02%ad%x02%an%x02%s",
        &max,
    ];
    cmd.extend(rev);
    cmd.push("--");
    cmd.extend(paths.iter().map(String::as_str));
    let commits = parse(&git(&cmd)?);
    if commits.is_empty() {
        if out.format != Format::Json {
            eprintln!("(no commits)");
        }
        return Ok(Outcome::Empty);
    }
    if out.format == Format::Json {
        let rows: Vec<_> = commits
            .iter()
            .map(|c| {
                serde_json::json!({
                    "hash": c.hash, "date": c.date, "author": c.author, "subject": c.subject,
                    "files": c.files, "added": c.add, "deleted": c.del,
                })
            })
            .collect();
        outln!("{}", serde_json::to_string(&rows)?);
        return Ok(Outcome::Found);
    }
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    let cut = a.cut.map(|n| n as usize);
    if out.format == Format::Human {
        print_human(&commits, out, &mut budget, cut)?;
    } else {
        let now = now();
        for c in &commits {
            let line = format!(
                "{} {} {}",
                out.header(&c.hash),
                c.subject,
                paint(&format!("({})", age(now, c.at)), Tone::Dim, out.color)
            );
            let line = cut.map_or_else(|| line.clone(), |n| clip(&line, 0, n));
            budget.emit(&line);
        }
        budget.emit(&out.header(&format!(
            "== {} commit{} (more: -n N, one commit's changes: acus diff REV^..REV)",
            commits.len(),
            if commits.len() == 1 { "" } else { "s" }
        )));
    }
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
    Ok(Outcome::Found)
}

/// Seconds since the Unix epoch, as git reports them.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A compact age for a commit timestamp: `3d ago`, `2h ago`, `just now`.
fn age(now: i64, at: i64) -> String {
    let secs = now.saturating_sub(at).max(0) as u64;
    let (n, unit) = match secs {
        0..=59 => return "just now".to_owned(),
        60..=3599 => (secs / 60, "m"),
        3600..=86_399 => (secs / 3600, "h"),
        86_400..=2_591_999 => (secs / 86_400, "d"),
        2_592_000..=31_535_999 => (secs / 2_592_000, "mo"),
        _ => (secs / 31_536_000, "y"),
    };
    format!("{n}{unit} ago")
}

/// The human view: a titled table of commits with aligned columns, then the follow-up commands.
fn print_human(
    commits: &[Commit],
    out: &Out,
    budget: &mut Budget,
    cut: Option<usize>,
) -> io::Result<()> {
    let mut doc = Doc::default();
    doc.title(format!(
        "{} commit{}",
        commits.len(),
        if commits.len() == 1 { "" } else { "s" }
    ));
    let rows: Vec<Vec<String>> = commits
        .iter()
        .map(|c| {
            let stat = match c.files {
                0 => String::new(),
                n => format!("{} {n}f", changed(c.add, c.del)),
            };
            [&c.hash, &c.date, &c.author, &stat, &c.subject]
                .into_iter()
                .map(|cell| cut.map_or_else(|| cell.clone(), |n| clip(cell, 0, n)))
                .collect()
        })
        .collect();
    doc.table(Table {
        head: ["commit", "date", "author", "changes", "subject"]
            .map(str::to_owned)
            .into(),
        align: vec![
            Align::Left,
            Align::Left,
            Align::Left,
            Align::Right,
            Align::Left,
        ],
        rows,
    });
    doc.line("more: -n N; one commit's changes: acus diff REV^..REV");
    out.doc_budget(&doc, budget)
}

fn changed(add: usize, del: usize) -> String {
    format!("+{add} -{del}")
}

/// Records start with \x01 and hold four \x02-separated fields, then `add\tdel\tpath` lines.
fn parse(log: &str) -> Vec<Commit> {
    log.split('\x01')
        .filter(|r| !r.trim().is_empty())
        .filter_map(|r| {
            let (head, stats) = r.split_once('\n').unwrap_or((r, ""));
            let mut f = head.splitn(5, '\x02');
            let mut c = Commit {
                hash: f.next()?.into(),
                at: f.next()?.parse().unwrap_or(0),
                date: f.next()?.into(),
                author: f.next()?.into(),
                subject: f.next()?.into(),
                files: 0,
                add: 0,
                del: 0,
            };
            for l in stats.lines() {
                let mut p = l.splitn(3, '\t');
                let (Some(x), Some(y), Some(_)) = (p.next(), p.next(), p.next()) else {
                    continue;
                };
                c.files += 1;
                // Binary files report `-` for both counts.
                c.add += x.parse().unwrap_or(0);
                c.del += y.parse().unwrap_or(0);
            }
            Some(c)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_numstat_per_commit_and_keeps_merges_empty() {
        let log = "\x01abc1234\x021700000000\x022026-01-02\x02Ada\x02Fix: a\tb\n1\t2\ta.rs\n-\t-\timg.png\n\n\x01def5678\x021700000000\x022026-01-01\x02Bob\x02Merge\n";
        let c = parse(log);
        assert_eq!(c.len(), 2);
        assert_eq!(
            (
                c[0].hash.as_str(),
                c[0].subject.as_str(),
                c[0].files,
                c[0].add,
                c[0].del
            ),
            ("abc1234", "Fix: a\tb", 2, 1, 2)
        );
        assert_eq!((c[1].files, c[1].add, c[1].del), (0, 0, 0));
    }

    #[test]
    fn age_is_compact() {
        assert_eq!(age(1000, 1000), "just now");
        assert_eq!(age(1120, 1000), "2m ago");
        assert_eq!(age(8200, 1000), "2h ago");
        assert_eq!(age(1000 + 2 * 86_400, 1000), "2d ago");
        assert_eq!(age(1000 + 40 * 86_400, 1000), "1mo ago");
        assert_eq!(age(1000 + 400 * 86_400, 1000), "1y ago");
    }
}
