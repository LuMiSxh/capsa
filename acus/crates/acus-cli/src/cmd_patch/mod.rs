// SPDX-License-Identifier: MPL-2.0
//! `acus patch`: applies a patch from stdin or `--file`, then reports what was written.

use crate::config::Config;
use crate::fmt::{Out, more_lines};
use acus_edit::{Change, FileText, Region};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use similar::{Algorithm, DiffOp, TextDiff, capture_diff_slices};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
}

/// Written lines shown per region; longer regions end with a `show` hint.
const REGION_LINES: usize = 20;
/// Region lines echoed per patch, so a large patch does not flood the output.
const ECHO_LINES: usize = 120;
/// Regions past the echo cap that are named by address, the rest are counted.
const NAMED_REGIONS: usize = 10;

#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out, nested: bool, cfg: &Config) -> Result<Outcome> {
    if nested && a.file.is_none() {
        return Err(capsa_core::hint::Hinted::new(
            "patch inside run needs --file",
            "write the patch to a file or call acus patch directly",
        )
        .into());
    }
    let text = read_patch(a.file.as_deref())?;
    if a.fmt && a.check {
        return Err(capsa_core::hint::Hinted::new(
            "--fmt cannot be combined with --check, which writes nothing",
            "drop --fmt to preview, or drop --check to format while applying",
        )
        .into());
    }
    let mut applied = acus_edit::apply_with(
        &acus_edit::parse(&text).hinted()?,
        Path::new(""),
        a.check,
        a.allow_broken,
    )
    .hinted()?;
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
    if !applied.broken.is_empty() {
        notes.push(format!(
            "note: --allow-broken: wrote {} with more syntax errors than before",
            applied.broken.join(", ")
        ));
    }
    notes.extend(
        applied.skipped.iter().map(|n| {
            format!("note: patch line {n}: SEARCH is identical to REPLACE; block skipped")
        }),
    );
    if a.diff {
        for t in &applied.texts {
            print!("{}", unified(t));
        }
        for l in &notes {
            println!("{l}");
        }
        return Ok(Outcome::Found);
    }
    if out.format == Format::Json {
        print_json(&applied.regions, &lines, &notes, !a.check);
        return Ok(Outcome::Found);
    }
    for l in &lines {
        println!("{l}");
    }
    for l in &notes {
        println!("{l}");
    }
    if !a.quiet {
        echo_regions(out, &applied.regions);
    }
    if a.check {
        println!("(check only, nothing written)");
    }
    Ok(Outcome::Found)
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
    println!(
        "{}",
        serde_json::json!({ "changes": lines, "notes": notes, "regions": regions, "written": written })
    );
}

/// Echoes the written regions up to the line cap; the rest are named as `acus show` addresses.
fn echo_regions(out: &Out, regions: &[Region]) {
    let mut echoed = 0;
    let mut unshown: Vec<String> = Vec::new();
    for r in regions {
        let end = r.start + r.lines.len() - 1;
        let shown = r.lines.len().min(REGION_LINES);
        // Once one region does not fit, the rest are named too, so the echo keeps patch order.
        if !unshown.is_empty() || echoed + shown > ECHO_LINES {
            unshown.push(format!("{}:{}-{end}", r.path, r.start));
            continue;
        }
        echoed += shown;
        println!(
            "{}",
            out.header(&format!("== {} {}-{end}", r.path, r.start))
        );
        for (i, l) in r.lines.iter().take(REGION_LINES).enumerate() {
            println!("{}", out.numbered(r.start + i, l, false));
        }
        if r.lines.len() > REGION_LINES {
            println!(
                "{}",
                more_lines(
                    &r.path,
                    r.start + REGION_LINES,
                    end,
                    r.lines.len() - REGION_LINES
                )
            );
        }
    }
    if !unshown.is_empty() {
        println!("{}", more_regions(&unshown));
    }
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
    for c in changes {
        let path = match c {
            Change::Added { path, .. } | Change::Modified { path, .. } => path,
            Change::Moved { to, .. } => to,
            Change::Deleted { .. } => continue,
        };
        let Some(argv) = formatter(Path::new(path)) else {
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

/// Formatter command reading the file on stdin and printing it formatted, if one is installed.
fn formatter(path: &Path) -> Option<Vec<String>> {
    let p = path.to_string_lossy().into_owned();
    let v = |a: &[&str]| Some(a.iter().map(|s| s.to_string()).collect());
    match path.extension()?.to_str()? {
        "rs" if on_path("rustfmt") => v(&["rustfmt", "--edition", &rust_edition(path)]),
        "py" if on_path("ruff") => v(&["ruff", "format", "--stdin-filename", &p, "-"]),
        "py" if on_path("black") => v(&["black", "-q", "--stdin-filename", &p, "-"]),
        "go" if on_path("gofmt") => v(&["gofmt"]),
        "swift" if on_path("swift-format") => {
            v(&["swift-format", "format", "--assume-filename", &p])
        }
        "swift" if cfg!(target_os = "macos") && on_path("xcrun") => {
            v(&["xcrun", "swift-format", "format", "--assume-filename", &p])
        }
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "svelte" | "vue" | "css"
        | "scss" => {
            let bin = path
                .canonicalize()
                .ok()?
                .ancestors()
                .map(|d| d.join("node_modules/.bin/prettier"))
                .find(|b| b.is_file())
                .map(|b| b.to_string_lossy().into_owned())
                .or_else(|| on_path("prettier").then(|| "prettier".into()))?;
            Some(vec![bin, "--stdin-filepath".into(), p])
        }
        _ => None,
    }
}

fn on_path(name: &str) -> bool {
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd"]
    } else {
        &[""]
    };
    std::env::var_os("PATH").is_some_and(|ps| {
        std::env::split_paths(&ps)
            .any(|d| exts.iter().any(|e| d.join(format!("{name}{e}")).is_file()))
    })
}

/// `edition` from the nearest Cargo.toml that sets one (following `edition.workspace`).
fn rust_edition(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for d in abs.ancestors().skip(1) {
        let Ok(t) = std::fs::read_to_string(d.join("Cargo.toml")) else {
            continue;
        };
        let ed = t.lines().map(str::trim).find_map(|l| {
            let v = l.strip_prefix("edition")?.trim_start().strip_prefix('=')?;
            Some(v.trim().trim_matches('"').to_owned())
        });
        if let Some(ed) = ed.filter(|e| e.chars().all(|c| c.is_ascii_digit())) {
            return ed;
        }
    }
    "2021".into()
}

fn pipe(argv: &[String], input: &str) -> Result<String> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("child stdin is not piped"))?;
    let input = input.to_owned();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child.wait_with_output()?;
    writer.join().ok();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!(
            "{}",
            err.lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
                .trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}
