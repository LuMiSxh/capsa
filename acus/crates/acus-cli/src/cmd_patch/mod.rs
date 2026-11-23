// SPDX-License-Identifier: MPL-2.0
//! `acus patch`: applies a patch from stdin or `--file`, then reports what was written.

mod formatter;
mod print;

use crate::cmd_run;
use crate::config::Config;
use crate::out::Out;
use acus_edit::{Applied, Change, FileText, Op, Region};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use capsa_core::hint::hinted;
use formatter::{Tools, command_for, pipe};
use serde::Serialize;
use similar::{Algorithm, DiffOp, TextDiff, capture_diff_slices};
use std::io::Read;
use std::path::{Path, PathBuf};

mod syntax;
#[cfg(test)]
mod tests;

use syntax::SYNTAX;

#[derive(clap::Args)]
#[command(after_long_help = SYNTAX)]
pub struct Args {
    /// Read the patch from this file instead of stdin.
    #[arg(short, long, value_parser = crate::home::<PathBuf>)]
    file: Option<PathBuf>,
    /// Validate and summarise the changes; write nothing.
    #[arg(long)]
    check: bool,
    /// Run CMD after the patch applies, as `acus run CMD` would, and print its output after the
    /// patch's; the exit code is CMD's. A failed patch runs nothing, and `--check` never runs CMD.
    #[arg(long, value_name = "CMD")]
    run: Option<String>,
    /// Print a unified diff instead of the written lines; with --check nothing is written.
    #[arg(long)]
    diff: bool,
    /// Print only the summary, not the written lines.
    #[arg(short, long)]
    quiet: bool,
    /// Run each written file through its formatter (rustfmt, ruff/black, prettier, swift-format,
    /// gofmt) if installed; the printed lines are the formatted ones.
    #[arg(long, overrides_with = "no_fmt")]
    fmt: bool,
    /// Skip formatting even if `[patch] fmt = true` is configured.
    #[arg(long)]
    no_fmt: bool,
    /// Write a file even if the patch gives it more syntax errors than it had.
    #[arg(long)]
    allow_broken: bool,
    /// Match SEARCH blocks that differ only in whitespace: tabs vs spaces, whitespace runs and
    /// blank lines; REPLACE is written as given, and two matches still fail.
    #[arg(long, overrides_with = "no_fuzzy")]
    fuzzy: bool,
    /// Skip fuzzy matching even if `[patch] fuzzy = true` is configured.
    #[arg(long)]
    no_fuzzy: bool,
}

/// `Replace Symbol` bodies longer than this echo only their first and last lines.
const SYMBOL_BODY_LINES: usize = 40;

/// What `acus patch` prints: the summary, the notes, the written lines and any diffs.
#[derive(Debug, Serialize)]
pub(crate) struct PatchReport {
    /// One summary line per change, phrased for `--check` when nothing was written, plus the
    /// formatter notes.
    changes: Vec<String>,
    /// Tolerant matches, skipped blocks and files written broken.
    notes: Vec<String>,
    /// Written line ranges and the lines to echo, in patch order.
    regions: Vec<WrittenRegion>,
    /// Unified diffs for `--diff`.
    diffs: Vec<String>,
    /// Whether the patch only validated, and whether the summary is the whole output.
    check: bool,
    quiet: bool,
    diff: bool,
    /// Whether the patch wrote to disk, for the `--json` document.
    written: bool,
}

/// A written line range and whether it is a long `Replace Symbol` body echoed at its edges.
#[derive(Debug, Serialize)]
pub(crate) struct WrittenRegion {
    path: String,
    start: usize,
    lines: Vec<String>,
    long: bool,
}

/// Applies the patch and gathers its report; writes the files but nothing to stdout.
pub(crate) fn compute(a: &Args, cfg: &Config) -> Result<PatchReport> {
    let text = read_patch(a.file.as_deref())?;
    compute_text(a, &text, cfg)
}

/// Applies the patch `text`, which the caller already has, as `compute` applies one read from
/// stdin.
pub(crate) fn compute_text(a: &Args, text: &str, cfg: &Config) -> Result<PatchReport> {
    refuse_fmt_with_check(a)?;
    compute_ops(a, &parse_ops(text)?, cfg)
}

fn refuse_fmt_with_check(a: &Args) -> Result<()> {
    if a.fmt && a.check {
        return Err(hinted(
            "--fmt cannot be combined with --check, which writes nothing",
            "drop --fmt to preview, or drop --check to format while applying",
        ));
    }
    Ok(())
}

/// Parses patch text into the ops `compute_ops` applies.
pub(crate) fn parse_ops(text: &str) -> Result<Vec<Op>> {
    acus_edit::parse(text).hinted()
}

/// Applies ops the caller already has, as `compute_text` applies the ones its text parses to.
pub(crate) fn compute_ops(a: &Args, ops: &[Op], cfg: &Config) -> Result<PatchReport> {
    refuse_fmt_with_check(a)?;
    let options = apply_options(a, cfg);
    let mut applied = acus_edit::apply(ops, Path::new(""), &options).hinted()?;
    let long_bodies = long_symbol_bodies(ops);
    let mut fmt_notes = Vec::new();
    if (a.fmt || cfg.patch.fmt) && !a.no_fmt && !a.check {
        fmt_notes = format_written(&applied.changes, &mut applied.regions, &mut applied.texts);
        applied.recount();
    }
    let mut changes: Vec<String> = applied
        .changes
        .iter()
        .map(|c| if a.check { would(c) } else { c.to_string() })
        .collect();
    changes.extend(fmt_notes);
    let diffs = if a.diff {
        applied.texts.iter().map(unified).collect()
    } else {
        Vec::new()
    };
    Ok(PatchReport {
        changes,
        notes: note_lines(&applied),
        regions: written_regions(applied.regions, &long_bodies),
        diffs,
        check: a.check,
        quiet: a.quiet,
        diff: a.diff,
        written: !a.check,
    })
}

/// Applies a patch, prints the written lines, then runs `--run`'s command when it applied.
pub fn run(a: &Args, out: &Out, cfg: &Config) -> Result<Outcome> {
    let report = compute(a, cfg)?;
    let command = a.run.as_deref().filter(|_| !a.check);
    if out.format == Format::Json
        && !report.diff
        && let Some(command) = command
    {
        let run_args = cmd_run::Args::for_command(command.to_owned());
        let run_report = cmd_run::compute(&run_args, out, cfg)?;
        outln!(
            "{}",
            serde_json::json!({
                "patch": print::json(&report),
                "run": cmd_run::json(&run_report)
            })
        );
        return Ok(cmd_run::outcome(&run_report));
    }
    let outcome = print::print(&report, out);
    match command {
        Some(command) => {
            let run_args = cmd_run::Args::for_command(command.to_owned());
            cmd_run::run(&run_args, out, cfg)
        }
        None => Ok(outcome),
    }
}

/// The notes on tolerant matches, skipped blocks and files written broken or unchecked.
fn note_lines(applied: &Applied) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    for (list, what) in [
        (&applied.numbered, "stripped line-number prefixes"),
        (&applied.trailing, "matched ignoring trailing whitespace"),
        (
            &applied.indented,
            "matched ignoring indentation (tabs vs spaces)",
        ),
        (&applied.crlf, "matched with CRLF line endings"),
    ] {
        if !list.is_empty() {
            notes.push(format!("note: {what} in {}", list.join(", ")));
        }
    }
    notes.extend(
        applied
            .fuzzy
            .iter()
            .map(|n| format!("note: matched fuzzily (whitespace): {n}")),
    );
    if !applied.broken.is_empty() {
        notes.push(format!(
            "note: --allow-broken: wrote {} with more syntax errors than before",
            applied.broken.join(", ")
        ));
    }
    if !applied.unchecked.is_empty() {
        notes.push(format!(
            "note: skipped the syntax check of {} (still has conflict markers)",
            applied.unchecked.join(", ")
        ));
    }
    notes.extend(
        applied.skipped.iter().map(|n| {
            format!("note: patch line {n}: SEARCH is identical to REPLACE; block skipped")
        }),
    );
    notes
}

/// The written regions with whether each is a long `Replace Symbol` body.
fn written_regions(regions: Vec<Region>, long_bodies: &[(&str, &[String])]) -> Vec<WrittenRegion> {
    regions
        .into_iter()
        .map(|r| {
            let long = long_bodies
                .iter()
                .any(|&(path, lines)| path == r.path && lines == r.lines.as_slice());
            WrittenRegion {
                path: r.path,
                start: r.start,
                lines: r.lines,
                long,
            }
        })
        .collect()
}

/// The apply options from the flags and the `[patch]` config.
fn apply_options(a: &Args, cfg: &Config) -> acus_edit::ApplyOptions {
    acus_edit::ApplyOptions {
        check: a.check,
        allow_broken: a.allow_broken,
        fuzzy: (a.fuzzy || cfg.patch.fuzzy) && !a.no_fuzzy,
    }
}

/// The patch text, from `--file` or standard input.
fn read_patch(file: Option<&Path>) -> Result<String> {
    Ok(match file {
        Some(f) => {
            std::fs::read_to_string(f).with_context(|| format!("cannot read {}", f.display()))?
        }
        None => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .context("cannot read stdin")?;
            s
        }
    })
}

/// The `Replace Symbol` bodies that are long enough to echo only their edges.
fn long_symbol_bodies(ops: &[Op]) -> Vec<(&str, &[String])> {
    ops.iter()
        .filter_map(|op| match op {
            Op::ReplaceSymbol { path, lines, .. } if lines.len() > SYMBOL_BODY_LINES => {
                Some((path.as_str(), lines.as_slice()))
            }
            _ => None,
        })
        .collect()
}

/// The summary of a change that `--check` did not write.
fn would(c: &Change) -> String {
    match c {
        Change::Added { path, lines } => format!("would add {path} +{lines}"),
        Change::Deleted { path, lines } => format!("would delete {path} -{lines}"),
        Change::Modified {
            path,
            added,
            removed,
        } => format!("would update {path} +{added} -{removed}"),
        Change::Moved {
            from,
            to,
            added,
            removed,
        } => format!("would move {from} -> {to} +{added} -{removed}"),
    }
}

/// A unified diff of one file; a side the patch creates or deletes is `/dev/null`.
fn unified(t: &FileText) -> String {
    let from = match &t.before {
        Some(_) => format!("a/{}", t.path),
        None => "/dev/null".into(),
    };
    let to = match &t.after {
        Some(_) => format!("b/{}", t.path),
        None => "/dev/null".into(),
    };
    TextDiff::from_lines(
        t.before.as_deref().unwrap_or(""),
        t.after.as_deref().unwrap_or(""),
    )
    .unified_diff()
    .context_radius(3)
    .header(&from, &to)
    .to_string()
}

/// Formats every written file and moves `regions` onto the formatted lines.
/// Returns one note per formatted file or failing formatter; failures keep the file as
/// written.
fn format_written(
    changes: &[Change],
    regions: &mut [Region],
    texts: &mut [FileText],
) -> Vec<String> {
    let mut notes = Vec::new();
    let tools = Tools::default();
    for c in changes {
        let path = match c {
            Change::Added { path, .. } | Change::Modified { path, .. } => path,
            Change::Moved { to, .. } => to,
            Change::Deleted { .. } => continue,
        };
        let Some(argv) = command_for(Path::new(path), &tools) else {
            continue;
        };
        let Ok(old) = std::fs::read_to_string(path) else {
            continue;
        };
        let new = match pipe(&argv, &old) {
            Ok(new) => new,
            Err(e) => {
                notes.push(format!("fmt {}: {path} left as written ({e})", argv[0]));
                continue;
            }
        };
        if new == old || new.trim().is_empty() {
            continue;
        }
        if let Err(e) = std::fs::write(path, &new) {
            notes.push(format!("fmt {}: cannot write {path} ({e})", argv[0]));
            continue;
        }
        if let Some(t) = texts.iter_mut().find(|t| t.path == *path) {
            t.after = Some(new.replace("\r\n", "\n"));
        }
        let (o, n): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
        let ops = capture_diff_slices(Algorithm::Myers, &o, &n);
        let mut mine: Vec<(usize, usize)> = Vec::new();
        for r in regions.iter_mut().filter(|r| r.path == *path) {
            let (s, e) = (r.start - 1, r.start - 1 + r.lines.len());
            mine.push((s, e));
            let (ns, ne) = (map(&ops, s, false), map(&ops, e - 1, true));
            r.start = ns + 1;
            r.lines = n[ns..ne.max(ns)].iter().map(|l| l.to_string()).collect();
        }
        let elsewhere: usize = ops
            .iter()
            .filter(|op| !matches!(op, DiffOp::Equal { .. }))
            .filter(|op| {
                let o = op.old_range();
                !mine.iter().any(|&(s, e)| o.start <= e && s <= o.end)
            })
            .map(|op| op.old_range().len().max(op.new_range().len()))
            .sum();
        let extra = if elsewhere > 0 {
            format!(", also {elsewhere} lines outside the patch")
        } else {
            String::new()
        };
        notes.push(format!("fmt {}: {path}{extra}", argv[0]));
    }
    notes
}

/// Where old line `i` ended up: its own new index, or the start/end of what replaced it.
fn map(ops: &[DiffOp], i: usize, end: bool) -> usize {
    for op in ops {
        let (o, n) = (op.old_range(), op.new_range());
        if o.contains(&i) {
            return match op {
                DiffOp::Equal { .. } => n.start + (i - o.start) + usize::from(end),
                _ if end => n.end,
                _ => n.start,
            };
        }
    }
    ops.last().map_or(0, |op| op.new_range().end)
}
