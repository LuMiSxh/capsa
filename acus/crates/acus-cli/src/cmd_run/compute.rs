// SPDX-License-Identifier: MPL-2.0
//! Gathers everything `acus run` prints, without writing to stdout.

use super::narrow::narrow;
use super::noise::{clean, quiet, save_log};
use super::refs::{refs, snippet};
use super::repeat::{self, Streak};
use super::{Args, Source, inputs, source};
use crate::config::Config;
use crate::out::Out;
use anyhow::Result;
use capsa_core::format::Format;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The command's run: its condensed output, the code its references point at and the notes print
/// puts around them.
#[derive(Debug, Default, Serialize)]
pub(crate) struct RunReport {
    /// The command the user did not pass, announced with `$` in the agent views.
    pub(super) announced: Option<String>,
    /// The command's exit code, also the process exit code.
    pub(super) code: i32,
    /// `--timeout` killed the command after these seconds.
    pub(super) killed_after: Option<u64>,
    /// Identical failures of this command in this session.
    pub(super) repeat: Option<Streak>,
    /// A `--log` input: its path and line count.
    pub(super) input_log: Option<(PathBuf, usize)>,
    /// Seconds since the `--log` input was last written, when it may still grow.
    pub(super) growing: Option<u64>,
    /// Lines kept after the noise filter, `--grep` and `--tail`.
    pub(super) lines: Vec<String>,
    /// Noise lines dropped, and tests that passed.
    pub(super) dropped: usize,
    pub(super) passed: usize,
    /// Lines `--grep` dropped, and whether any line matched it.
    pub(super) grep_dropped: usize,
    pub(super) nothing_matched: bool,
    /// Lines `--tail` dropped.
    pub(super) tail_dropped: usize,
    /// Lines left out between the head and tail halves of `--max-output`.
    pub(super) omitted: usize,
    /// The full output saved for a rerun: its path and line count.
    pub(super) saved_log: Option<(PathBuf, usize)>,
    /// The code at each reference, grouped by snippet.
    pub(super) snippets: Vec<Snippet>,
    /// References past `--max-refs`.
    pub(super) more_locations: usize,
    /// `--head` budget and the line cap that decides the head and tail halves.
    pub(super) head: Option<usize>,
    pub(super) max_output: usize,
    /// `--max-refs`, named in the footer for the locations it left out.
    pub(super) max_refs: usize,
    /// `--cut`, which clips each printed line.
    pub(super) cut: Option<u64>,
    /// The `--json` document's content.
    pub(super) json: Option<JsonView>,
}

/// The code one reference points at, with every referenced line marked.
#[derive(Debug, Serialize)]
pub(crate) struct Snippet {
    pub(super) label: String,
    pub(super) start: usize,
    pub(super) lines: Vec<String>,
    pub(super) marks: Vec<usize>,
}

/// The shown output and the code it references, as the `--json` document prints them.
#[derive(Debug, Serialize)]
pub(crate) struct JsonView {
    pub(super) shown: String,
    pub(super) locations: Vec<Location>,
}

/// One entry of the `--json` document's `locations` array.
#[derive(Debug, Serialize)]
pub(crate) struct Location {
    pub(super) address: String,
    pub(super) start: usize,
    pub(super) text: String,
}

/// Runs the command (or reads the log) and gathers its report; writes nothing to stdout.
pub(crate) fn compute(a: &Args, out: &Out, cfg: &Config) -> Result<RunReport> {
    let (extra, grep, context) = inputs(a, cfg)?;
    let narrowing = a.tail.is_some() || grep.is_some();
    let Source {
        text: raw,
        code,
        timed_out,
        command,
        announced,
    } = source(a, cfg)?;
    let text = if a.raw { raw } else { clean(&raw) };
    let cwd = std::env::current_dir()?;
    let all: Vec<&str> = text.lines().collect();
    let (lines, passed, dropped) = if a.raw {
        (all.iter().map(|l| l.to_string()).collect(), 0, 0)
    } else {
        quiet(&all, &extra)
    };
    let repeat = command
        .as_deref()
        .and_then(|c| repeat::note(&cwd, c, code, &lines));
    let mut report = RunReport {
        announced,
        code,
        killed_after: if timed_out { a.timeout } else { None },
        repeat,
        input_log: a.log.as_ref().map(|p| (p.clone(), all.len())),
        growing: a.log.as_deref().and_then(growing_age),
        head: a.head.map(|n| n as usize),
        max_output: a.max_output,
        max_refs: a.max_refs,
        cut: a.cut,
        ..RunReport::default()
    };
    if out.format == Format::Json {
        let shown = if narrowing {
            let lines = text.lines().map(str::to_owned).collect();
            narrow(
                lines,
                (a.tail, grep.as_ref(), context),
                (a.max_output, a.max_lines),
            )
            .lines
            .join("\n")
        } else {
            text
        };
        report.json = Some(JsonView {
            locations: locations(&shown, a, &cwd),
            shown,
        });
        return Ok(report);
    }
    let nothing_matched = grep
        .as_ref()
        .is_some_and(|re| !lines.is_empty() && !lines.iter().any(|l| re.is_match(l)));
    let narrowed = narrow(
        lines,
        (a.tail, grep.as_ref(), context),
        (a.max_output, a.max_lines),
    );
    // Only what is shown decides which code is shown.
    let found = if narrowing {
        refs(&narrowed.lines.join("\n"), &cwd)
    } else {
        refs(&text, &cwd)
    };
    let (snippets, more_locations) = group_snippets(found, a);
    let omitted = omitted_count(a, narrowed.lines.len());
    // Whatever is not printed stays readable without running the command again.
    // Dropped noise matters only when looking into a failure.
    let keep_log = omitted > 0
        || narrowed.grep_dropped + narrowed.tail_dropped > 0
        || timed_out
        || (dropped > 0 && code != 0);
    let saved_log = command
        .as_deref()
        .filter(|_| keep_log)
        .and_then(|c| save_log(&cwd, c, &text))
        .map(|p| (p, text.lines().count()));
    report.lines = narrowed.lines;
    report.dropped = dropped;
    report.passed = passed;
    report.grep_dropped = narrowed.grep_dropped;
    report.tail_dropped = narrowed.tail_dropped;
    report.nothing_matched = nothing_matched;
    report.snippets = snippets;
    report.more_locations = more_locations;
    report.omitted = omitted;
    report.saved_log = saved_log;
    Ok(report)
}

/// The `--json` locations: the code behind the first `--max-refs` references.
fn locations(shown: &str, a: &Args, cwd: &Path) -> Vec<Location> {
    refs(shown, cwd)
        .iter()
        .take(a.max_refs)
        .filter_map(|(p, l)| snippet(p, *l, a))
        .map(|(address, start, text)| Location {
            address,
            start,
            text: text.join("\n"),
        })
        .collect()
}

/// The snippets the references point at, one per enclosing symbol or nearby block, capped at
/// `--max-refs`; returns how many references the cap left out.
fn group_snippets(found: Vec<(PathBuf, usize)>, a: &Args) -> (Vec<Snippet>, usize) {
    let mut shown: Vec<Snippet> = Vec::new();
    let mut more = 0;
    for (path, line) in found {
        let Some((label, start, lines)) = snippet(&path, line, a) else {
            continue;
        };
        if let Some(s) = shown
            .iter_mut()
            .find(|s| s.label == label && s.start == start)
        {
            s.marks.push(line);
        } else if shown.len() < a.max_refs {
            shown.push(Snippet {
                label,
                start,
                lines,
                marks: vec![line],
            });
        } else {
            more += 1;
        }
    }
    (shown, more)
}

/// Lines between the head and tail halves that `--max-output` leaves out.
fn omitted_count(a: &Args, total: usize) -> usize {
    if a.tail.is_some() {
        0
    } else {
        let half = a.max_output.max(2) / 2;
        total.saturating_sub(half * 2)
    }
}

/// Seconds since `path` was last written, when it may still be growing.
fn growing_age(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .filter(|age| age.as_secs() < 10)
        .map(|age| age.as_secs())
}
