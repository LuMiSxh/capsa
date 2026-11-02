// SPDX-License-Identifier: MPL-2.0
//! `acus blame`: who last changed each run of lines, from `git blame --porcelain`.

use crate::fmt::Out;
use crate::git::git;
use crate::source::{Span, locate, read_text};
use acus_syntax::Address;
use anyhow::{Result, bail};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use std::collections::HashMap;
use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    /// path#Symbol, path:START-END, path:N-, path:LINE or path:-N, as in `acus show`. Blame
    /// reads the working tree, so `REV:path` is not accepted.
    #[arg(value_parser = crate::home::<String>)]
    addr: String,
}

/// Consecutive lines last changed by one commit.
struct Run {
    start: usize,
    end: usize,
    hash: String,
}

/// The header fields `git blame --porcelain` sends the first time a commit appears.
#[derive(Default)]
struct Commit {
    author: String,
    time: i64,
    tz: String,
    summary: String,
}

/// `git blame` of the lines an address names: one line per run of lines from one commit.
#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let addr = Address::parse(&a.addr);
    let path = addr.path.as_str();
    let src = read_text(path)?;
    let lines: Vec<&str> = src.lines().collect();
    if lines.is_empty() {
        bail!("{path} is empty");
    }
    let Span { start, end, .. } = locate(&addr, path, &src, &lines, &mut HashMap::new())?;
    let range = format!("{start},{}", end.min(lines.len()));
    // git blames from the file's own directory, so an absolute path works from any directory.
    let file = Path::new(path);
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .and_then(Path::to_str)
        .unwrap_or(".");
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or(path);
    let (runs, commits) = parse(&git(&[
        "-C",
        dir,
        "blame",
        "--porcelain",
        "-L",
        &range,
        "--",
        name,
    ])?);
    if out.format == Format::Json {
        let rows: Vec<_> = runs
            .iter()
            .map(|r| {
                let c = &commits[&r.hash];
                serde_json::json!({
                    "start": r.start, "end": r.end, "commit": r.hash, "author": c.author,
                    "date": date(c), "summary": c.summary,
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(rows));
        return Ok(Outcome::Found);
    }
    for r in &runs {
        let c = &commits[&r.hash];
        println!(
            "{}-{}  {}  {}  {}  {}",
            r.start,
            r.end,
            &r.hash[..8],
            c.author,
            date(c),
            c.summary
        );
    }
    Ok(Outcome::Found)
}

/// Splits `git blame --porcelain` into runs and the header fields of each commit.
fn parse(text: &str) -> (Vec<Run>, HashMap<String, Commit>) {
    let mut runs: Vec<Run> = Vec::new();
    let mut commits: HashMap<String, Commit> = HashMap::new();
    let mut hash = String::new();
    for l in text.lines() {
        // A source line; the header above it already names its commit.
        if l.starts_with('\t') {
            continue;
        }
        let mut words = l.split(' ');
        let first = words.next().unwrap_or_default();
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            // `<sha> <original line> <final line> [<lines in group>]`
            let line: usize = words.nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            hash = first.to_string();
            commits.entry(hash.clone()).or_default();
            match runs.last_mut() {
                Some(r) if r.hash == hash && r.end + 1 == line => r.end = line,
                _ => runs.push(Run {
                    start: line,
                    end: line,
                    hash: hash.clone(),
                }),
            }
        } else if let Some(c) = commits.get_mut(&hash) {
            if let Some(v) = l.strip_prefix("author ") {
                c.author = v.into();
            } else if let Some(v) = l.strip_prefix("author-time ") {
                c.time = v.parse().unwrap_or(0);
            } else if let Some(v) = l.strip_prefix("author-tz ") {
                c.tz = v.into();
            } else if let Some(v) = l.strip_prefix("summary ") {
                c.summary = v.into();
            }
        }
    }
    // Lines git has not committed yet are blamed on the all-zero commit; name them as such.
    for (hash, c) in &mut commits {
        if hash.bytes().all(|b| b == b'0') {
            c.author = "-".into();
            c.summary = "uncommitted".into();
        }
    }
    (runs, commits)
}

/// The author's date as `git log --date=short` prints it: `YYYY-MM-DD` in the author's
/// time zone.
fn date(c: &Commit) -> String {
    let digits: i64 = c.tz.get(1..).and_then(|d| d.parse().ok()).unwrap_or(0);
    let sign = if c.tz.starts_with('-') { -1 } else { 1 };
    let offset = sign * ((digits / 100) * 3600 + (digits % 100) * 60);
    let (y, m, d) = civil((c.time + offset).div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Civil (proleptic Gregorian) date of a day count since 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_follow_the_author_time_zone() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
        let c = |tz: &str| Commit {
            time: 1_709_251_199,
            tz: tz.into(),
            ..Default::default()
        };
        assert_eq!(date(&c("+0100")), "2024-03-01");
        assert_eq!(date(&c("-0500")), "2024-02-29");
    }
}
