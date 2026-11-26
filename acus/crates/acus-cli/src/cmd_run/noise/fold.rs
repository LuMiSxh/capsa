// SPDX-License-Identifier: MPL-2.0
//! Folds repeated lines and repeated multi-line summary blocks, keeping the first and last of a
//! run and never folding a diagnostic.

use super::patterns::{Category, Stats};
use regex::Regex;
use std::sync::LazyLock;

/// A status line carries a duration or a progress counter. Only such runs fold by head word,
/// so warnings, file listings and free-form output keep every line.
#[expect(clippy::unwrap_used, reason = "a constant pattern")]
static STATUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\s*[\d.]+\s*(?:ms|s)\s*\]|\(\s*[\d.]+\s*(?:ms|s)\s*\)|\(\s*\d+/\d+\s*\)")
        .unwrap()
});

/// Collapses the kept lines: repeated lines, runs of similar status lines, and the nested
/// `Test Suite`/`Executed` summaries XCTest prints at every level.
pub(super) fn collapse(lines: &[&str], stats: &mut Stats) -> Vec<String> {
    fold_suites(&fold_runs(lines, stats), stats)
}

/// The first word of a line, the part a run of status lines has in common.
fn head(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

/// A head or line that reports a problem, which no fold may hide.
fn diagnostic(line: &str) -> bool {
    let h = head(line).to_ascii_lowercase();
    [
        "error",
        "warn",
        "fail",
        "fatal",
        "panic",
        "assert",
        "exception",
        "traceback",
    ]
    .iter()
    .any(|w| h.starts_with(w))
        || line.contains("FAILED")
        || line.contains("panicked")
}

/// True when `b` continues the run `a` starts: the same line, or another status line of the same
/// tool with the same head word, such as two `PASS [ 0.01s] …` lines.
fn similar(a: &str, b: &str) -> bool {
    a == b || (head(a) == head(b) && STATUS.is_match(a) && STATUS.is_match(b))
}

/// Folds identical lines into `line (×N)` and a run of similar status lines into its first and
/// last line plus a count. A run with a diagnostic stays whole.
fn fold_runs(lines: &[&str], stats: &mut Stats) -> Vec<String> {
    const MIN_RUN: usize = 4;
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && !lines[j].trim().is_empty() && similar(lines[i], lines[j]) {
            j += 1;
        }
        let run = &lines[i..j];
        let len = run.len();
        if len == 1 {
            out.push(run[0].to_string());
        } else if run.iter().all(|l| *l == run[0]) {
            stats.add_n(Category::Duplicate, len - 1);
            out.push(format!("{} (×{len})", run[0]));
        } else if len >= MIN_RUN && !run.iter().any(|l| diagnostic(l)) {
            stats.add_n(Category::Fold, len - 2);
            out.push(run[0].to_string());
            out.push(format!("… {} similar lines", len - 2));
            out.push(run[len - 1].to_string());
        } else {
            out.extend(run.iter().map(|l| (*l).to_string()));
        }
        i = j;
    }
    out
}

/// The suite name of `Test Suite 'FooTests' passed at …`.
fn suite_name(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("Test Suite '")?;
    let (name, tail) = rest.split_once('\'')?;
    tail.contains(" at ").then_some(name)
}

/// True when `next` is the suite that encloses `prev`, such as `FooTests.xctest` around
/// `FooTests` or the root `All tests`.
fn nested(prev: &str, next: &str) -> bool {
    next.contains(prev) || next == "All tests"
}

/// The counts of `Executed 3 tests, with 1 failure (0 unexpected) in 0.003 (0.003) seconds`.
fn executed_counts(line: &str) -> &str {
    line.split_once(" in ")
        .map_or(line, |(counts, _)| counts)
        .trim()
}

/// Keeps the last pair of a nested XCTest summary run, so the same totals are shown once.
fn fold_suites(lines: &[String], stats: &mut Stats) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let start = i;
        let mut j = i;
        let mut prev_name: Option<String> = None;
        let mut counts: Option<String> = None;
        while j + 1 < lines.len() {
            let Some(name) = suite_name(&lines[j]) else {
                break;
            };
            if !lines[j + 1].trim_start().starts_with("Executed ") {
                break;
            }
            if prev_name.as_deref().is_some_and(|prev| !nested(prev, name)) {
                break;
            }
            let now = executed_counts(&lines[j + 1]);
            if counts.as_deref().is_some_and(|prev| prev != now) {
                break;
            }
            counts = Some(now.to_owned());
            prev_name = Some(name.to_owned());
            j += 2;
        }
        if j - start >= 4 {
            stats.add_n(Category::Fold, j - start - 2);
            out.push(lines[j - 2].clone());
            out.push(lines[j - 1].clone());
        } else {
            out.extend(lines[start..j].iter().cloned());
        }
        if j == start {
            out.push(lines[i].clone());
            i += 1;
        } else {
            i = j;
        }
    }
    out
}
