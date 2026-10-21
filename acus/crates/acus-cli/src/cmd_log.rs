// SPDX-License-Identifier: MPL-2.0
//! `acus log`: a compact git log, with the changed lines of each commit.

use crate::fmt::Out;
use crate::git::git;
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    /// Optional revision (default HEAD, or `A..B`), then paths to limit to.
    #[arg(value_parser = crate::home::<String>)]
    args: Vec<String>,
    /// Number of commits.
    #[arg(short = 'n', long, default_value_t = 15)]
    max_count: usize,
    /// Accepted for git habits: one line per commit is the only format.
    #[arg(long = "oneline", hide = true)]
    _oneline: bool,
}

struct Commit {
    hash: String,
    date: String,
    author: String,
    subject: String,
    files: usize,
    add: usize,
    del: usize,
}

/// Compact `git log`: one line per commit with date, author and changed lines.
#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out) -> Result<Outcome> {
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
        "--format=%x01%h%x02%ad%x02%an%x02%s",
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
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(Outcome::Found);
    }
    for c in &commits {
        let stat = match c.files {
            0 => String::new(),
            n => format!(" {} {n}f", changed(c.add, c.del)),
        };
        println!(
            "{} {} {} {}{}",
            out.header(&c.hash),
            c.date,
            c.author,
            c.subject,
            out.dim(&stat)
        );
    }
    println!(
        "{}",
        out.header(&format!(
            "== {} commit{} (more: -n N, one commit's changes: acus diff REV^..REV)",
            commits.len(),
            if commits.len() == 1 { "" } else { "s" }
        ))
    );
    Ok(Outcome::Found)
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
            let mut f = head.splitn(4, '\x02');
            let mut c = Commit {
                hash: f.next()?.into(),
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
        let log = "\x01abc1234\x022026-01-02\x02Ada\x02Fix: a\tb\n1\t2\ta.rs\n-\t-\timg.png\n\n\x01def5678\x022026-01-01\x02Bob\x02Merge\n";
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
}
