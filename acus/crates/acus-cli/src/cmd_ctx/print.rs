// SPDX-License-Identifier: MPL-2.0
//! Prints `acus ctx` results: the command's lines, its notes, code snippets and growing logs.

use super::Args;
use super::refs::{refs, snippet};
use crate::out::{Budget, Out, clip};
use capsa_core::exit::Outcome;
use capsa_core::render::{Tone, paint};
use std::path::{Path, PathBuf};

/// Returns the `--json` document: the shown output, the exit code and the code at each
/// reference.
pub(super) fn print_json(a: &Args, cwd: &Path, shown: &str, code: i32) -> Outcome {
    let locs: Vec<_> = refs(shown, cwd)
        .iter()
        .take(a.max_refs)
        .filter_map(|(p, l)| snippet(p, *l, a))
        .map(|s| serde_json::json!({"address": s.0, "start": s.1, "text": s.2.join("\n")}))
        .collect();
    outln!(
        "{}",
        serde_json::json!({"exit": code, "output": shown, "locations": locs})
    );
    Outcome::Code(code.clamp(0, 255) as u8)
}

/// Clips a printed line for `--cut`, as find and show do.
fn cut_line(line: &str, cut: Option<u64>) -> String {
    cut.map_or_else(|| line.to_owned(), |n| clip(line, 0, n as usize))
}

/// Prints the lines, or their head and tail with the gap noted. `--tail` already bounds the
/// output, and a cut must not hide the failures it keeps. Returns the number of lines left out.
pub(super) fn print_lines(a: &Args, lines: &[String], budget: &mut Budget) -> usize {
    let half = a.max_output.max(2) / 2;
    let cut = |l: &String| cut_line(l, a.cut);
    let omitted = if a.tail.is_some() {
        0
    } else {
        lines.len().saturating_sub(half * 2)
    };
    if omitted > 0 {
        lines[..half].iter().map(cut).for_each(|l| budget.emit(&l));
        budget.emit(&format!("… {omitted} output lines omitted"));
        lines[lines.len() - half..]
            .iter()
            .map(cut)
            .for_each(|l| budget.emit(&l));
    } else {
        lines.iter().map(cut).for_each(|l| budget.emit(&l));
    }
    omitted
}

/// Notes on what was dropped, and where the full output was saved: `log` is the path and the
/// line count.
pub(super) fn print_notes(
    out: &Out,
    dropped: usize,
    passed: usize,
    grep_dropped: usize,
    nothing_matched: bool,
    log: Option<(&Path, usize)>,
    budget: &mut Budget,
) {
    if dropped > 0 {
        let tests = if passed > 0 {
            format!("; {passed} tests passed")
        } else {
            String::new()
        };
        budget.emit(&paint(
            &format!("… {dropped} noise lines dropped{tests} (--raw keeps them)"),
            Tone::Dim,
            out.color,
        ));
    }
    if grep_dropped > 0 {
        let why = if nothing_matched && dropped > 0 {
            "; nothing matched, and the noise filter dropped lines (--raw matches those too)"
        } else if nothing_matched {
            "; nothing matched"
        } else {
            ""
        };
        budget.emit(&paint(
            &format!("… {grep_dropped} lines not matching --grep dropped{why}"),
            Tone::Dim,
            out.color,
        ));
    }
    if let Some((log, n)) = log {
        let p = log.display();
        budget.emit(&paint(
            &format!("… full output ({n} lines): acus find 'PAT' {p}, or acus show {p}:A-B"),
            Tone::Dim,
            out.color,
        ));
    }
}

/// The code at each reference: the enclosing symbol, or a few lines around it. References into
/// the same snippet print it once, with every referenced line marked.
pub(super) fn print_snippets(out: &Out, a: &Args, refs: &[(PathBuf, usize)], budget: &mut Budget) {
    let mut shown: Vec<(String, usize, Vec<String>, Vec<usize>)> = Vec::new();
    let mut more = 0;
    for (p, l) in refs {
        let Some((label, start, body)) = snippet(p, *l, a) else {
            continue;
        };
        if let Some(s) = shown.iter_mut().find(|s| s.0 == label && s.1 == start) {
            s.3.push(*l);
        } else if shown.len() < a.max_refs {
            shown.push((label, start, body, vec![*l]));
        } else {
            more += 1;
        }
    }
    for (label, start, body, marks) in &shown {
        let ls: Vec<String> = marks.iter().map(ToString::to_string).collect();
        let word = if marks.len() > 1 { "lines" } else { "line" };
        budget.emit(&out.header(&format!("== {label} ({word} {})", ls.join(", "))));
        for (i, t) in body.iter().enumerate() {
            budget.emit(&out.numbered(
                start + i,
                &cut_line(t, a.cut),
                marks.contains(&(start + i)),
            ));
        }
    }
    if more > 0 {
        budget.emit(&format!(
            "… {more} more locations (--max-refs {})",
            a.max_refs + more
        ));
    }
}

/// A saved log last written under 10 s ago may still be growing; the note says so.
pub(super) fn print_growing(out: &Out, path: &Path, budget: &mut Budget) {
    let Some(age) = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .filter(|age| age.as_secs() < 10)
    else {
        return;
    };
    let p = path.display();
    budget.emit(&paint(
        &format!(
            "{p} was written to {}s ago and may still be growing; rerun acus ctx --log {p} for newer lines",
            age.as_secs()
        ),
        Tone::Dim,
        out.color,
    ));
}
