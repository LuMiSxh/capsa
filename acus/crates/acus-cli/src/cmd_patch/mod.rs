// SPDX-License-Identifier: MPL-2.0
//! `acus patch`: applies a patch from stdin or `--file`, then reports what was written.

mod formatter;

use crate::config::Config;
use crate::out::{Out, more_lines};
use acus_edit::{Change, FileText, Op, Region};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use capsa_core::hint::hinted;
use formatter::{Tools, command_for, pipe};
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

/// Written lines shown per region; longer regions end with a `show` hint.
const REGION_LINES: usize = 20;
/// Region lines echoed per patch, so a large patch does not flood the output.
const ECHO_LINES: usize = 120;
/// Regions past the echo cap that are named by address, the rest are counted.
const NAMED_REGIONS: usize = 10;
/// `Replace Symbol` bodies longer than this echo only their first and last lines.
const SYMBOL_BODY_LINES: usize = 40;
/// Lines kept at each end of a long `Replace Symbol` body.
const SYMBOL_EDGE_LINES: usize = 5;

pub fn run(a: &Args, out: &Out, nested: bool, cfg: &Config) -> Result<Outcome> {
    if nested && a.file.is_none() {
        return Err(hinted(
            "patch inside run needs --file",
            "write the patch to a file or call acus patch directly",
        ));
    }
    let text = read_patch(a.file.as_deref())?;
    if a.fmt && a.check {
        return Err(hinted(
            "--fmt cannot be combined with --check, which writes nothing",
            "drop --fmt to preview, or drop --check to format while applying",
        ));
    }
    let ops = acus_edit::parse(&text).hinted()?;
    let options = apply_options(a, cfg);
    let mut applied = acus_edit::apply(&ops, Path::new(""), &options).hinted()?;
    let long_bodies = long_symbol_bodies(&ops);
    let mut fmt_notes = Vec::new();
    if (a.fmt || cfg.patch.fmt) && !a.no_fmt && !a.check {
        fmt_notes = format_written(&applied.changes, &mut applied.regions, &mut applied.texts);
        applied.recount();
    }
    let mut lines: Vec<String> = applied
        .changes
        .iter()
        .map(|c| if a.check { would(c) } else { c.to_string() })
        .collect();
    lines.extend(fmt_notes);
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
    if a.diff {
        for t in &applied.texts {
            outprint!("{}", unified(t));
        }
        for l in &notes {
            outln!("{l}");
        }
        return Ok(Outcome::Found);
    }
    if out.format == Format::Json {
        print_json(&applied.regions, &lines, &notes, !a.check);
        return Ok(Outcome::Found);
    }
    for l in &lines {
        outln!("{l}");
    }
    for l in &notes {
        outln!("{l}");
    }
    if !a.quiet {
        echo_regions(out, &applied.regions, &long_bodies);
    }
    if a.check {
        outln!("(check only, nothing written)");
    }
    Ok(Outcome::Found)
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

/// The `--format json` document: the changes, the notes and the written regions.
fn print_json(regions: &[Region], lines: &[String], notes: &[String], written: bool) {
    let regions: Vec<_> = regions
        .iter()
        .map(|r| {
            serde_json::json!({
                "path": r.path,
                "start": r.start,
                "end": r.start + r.lines.len() - 1,
                "text": r.lines.join("\n"),
            })
        })
        .collect();
    outln!(
        "{}",
        serde_json::json!({ "changes": lines, "notes": notes, "regions": regions, "written": written })
    );
}

/// Echoes the written regions up to the line cap; the rest are named as `acus show` addresses.
fn echo_regions(out: &Out, regions: &[Region], long_bodies: &[(&str, &[String])]) {
    let mut echoed = 0;
    let mut unshown: Vec<String> = Vec::new();
    for r in regions {
        let end = r.start + r.lines.len() - 1;
        let long = long_bodies
            .iter()
            .any(|&(path, lines)| path == r.path && lines == r.lines.as_slice());
        let shown = if long {
            2 * SYMBOL_EDGE_LINES
        } else {
            r.lines.len().min(REGION_LINES)
        };
        // Once one region does not fit, the rest are named too, so the echo keeps patch order.
        if !unshown.is_empty() || echoed + shown > ECHO_LINES {
            unshown.push(format!("{}:{}-{end}", r.path, r.start));
            continue;
        }
        echoed += shown;
        outln!(
            "{}",
            out.header(&format!("== {} {}-{end}", r.path, r.start))
        );
        for line in region_lines(out, r, long) {
            outln!("{line}");
        }
    }
    if !unshown.is_empty() {
        outln!("{}", more_regions(&unshown));
    }
}

/// The printed lines of a region: all of it, or the edges of a long `Replace Symbol` body.
fn region_lines(out: &Out, r: &Region, long: bool) -> Vec<String> {
    let n = r.lines.len();
    let end = r.start + n - 1;
    let numbered = |i: usize| out.numbered(r.start + i, &r.lines[i], false);
    if long {
        let edge = SYMBOL_EDGE_LINES;
        let mut lines: Vec<String> = (0..edge).map(numbered).collect();
        lines.push(more_lines(
            &r.path,
            r.start + edge,
            end - edge,
            n - 2 * edge,
        ));
        lines.extend((n - edge..n).map(numbered));
        lines
    } else {
        let mut lines: Vec<String> = (0..n.min(REGION_LINES)).map(numbered).collect();
        if n > REGION_LINES {
            lines.push(more_lines(
                &r.path,
                r.start + REGION_LINES,
                end,
                n - REGION_LINES,
            ));
        }
        lines
    }
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

/// The footer for regions the echo cap left out, as `acus show` addresses.
fn more_regions(unshown: &[String]) -> String {
    let named: Vec<&str> = unshown
        .iter()
        .take(NAMED_REGIONS)
        .map(String::as_str)
        .collect();
    let more = if unshown.len() > NAMED_REGIONS {
        " …"
    } else {
        ""
    };
    let regions = if unshown.len() == 1 {
        "region"
    } else {
        "regions"
    };
    format!(
        "… {} more written {regions}: acus show {}{more}",
        unshown.len(),
        named.join(" ")
    )
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
