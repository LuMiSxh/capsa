// SPDX-License-Identifier: MPL-2.0
//! Prints `acus run` results: the command's lines, its notes, code snippets and growing logs.

use super::compute::RunReport;
use crate::out::{Budget, Out, clip};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::render::{Tone, paint};

/// Prints the report in the format `out` asks for and returns the command's exit code.
pub(crate) fn print(report: &RunReport, out: &Out) -> Outcome {
    if out.format == Format::Json {
        return print_json(report);
    }
    if let Some(command) = &report.announced {
        outln!("{}", paint(&format!("$ {command}"), Tone::Dim, out.color));
    }
    let code = report.code;
    let mut budget = Budget::new(report.head);
    // The repeat line leads what the command printed, so it stays outside the `--head` budget and
    // that output is cut exactly as without it.
    if let Some(s) = &report.repeat {
        outln!("{}", s.lead());
    }
    // A saved log opens with its header, before any of its lines, so the first line says what
    // was read.
    if let Some((path, lines)) = &report.input_log {
        budget.emit(&out.header(&format!("== log {} ({lines} lines)", path.display())));
    }
    print_lines(report, &mut budget);
    print_notes(out, report, &mut budget);
    if let Some(secs) = report.killed_after {
        budget.emit(&paint(
            &format!(
                "… killed after {secs}s (--timeout); output above is what it printed until then. Raise --timeout, or --grep/--tail a narrower command"
            ),
            Tone::Dim,
            out.color,
        ));
    }
    if report.input_log.is_none() {
        budget.emit(&out.header(&format!("== exit {code}")));
    }
    print_snippets(out, report, &mut budget);
    print_growing(out, report, &mut budget);
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
    outcome(report)
}

/// Prints the `--json` document and returns the command's exit code.
fn print_json(report: &RunReport) -> Outcome {
    outln!("{}", json(report));
    outcome(report)
}

/// The `--json` document: the shown output, the exit code, the code at each reference, and
/// `repeat` (count and key line) from the third identical failure on.
pub(crate) fn json(report: &RunReport) -> serde_json::Value {
    let mut doc = serde_json::json!({"exit": report.code, "output": "", "locations": []});
    if let Some(view) = &report.json {
        doc["output"] = serde_json::json!(view.shown);
        doc["locations"] = serde_json::json!(view.locations);
    }
    if let Some(s) = &report.repeat {
        doc["repeat"] = serde_json::json!({"count": s.count, "key": s.key});
    }
    doc["summary"] = serde_json::json!(report.summary);
    if let Some(profile) = &report.profile {
        doc["profile"] = serde_json::json!(profile);
    }
    if report.explain && !report.stats.is_empty() {
        doc["noise"] = serde_json::json!(report.stats);
    }
    doc
}

/// Returns the outcome a report exits with: the command's code, clamped to 0-255.
pub(crate) fn outcome(report: &RunReport) -> Outcome {
    Outcome::Code(report.code.clamp(0, 255) as u8)
}

/// Clips a printed line for `--cut`, as find and show do.
fn cut_line(line: &str, cut: Option<u64>) -> String {
    cut.map_or_else(|| line.to_owned(), |n| clip(line, 0, n as usize))
}

/// Prints the kept lines; `--max-output` already kept every failure block and the tail.
fn print_lines(report: &RunReport, budget: &mut Budget) {
    let cut = |l: &String| cut_line(l, report.cut);
    report.lines.iter().map(cut).for_each(|l| budget.emit(&l));
}

/// Notes on what was dropped, and where the full output was saved.
fn print_notes(out: &Out, report: &RunReport, budget: &mut Budget) {
    if report.dropped > 0 {
        let s = report.summary;
        let tests = match (s.passed, s.skipped, s.failed) {
            (0, 0, 0) => String::new(),
            (1, 0, 0) => "; 1 test passed".to_owned(),
            (p, 0, 0) => format!("; {p} tests passed"),
            (0, k, 0) => format!("; {k} skipped"),
            (p, k, 0) => format!("; {p} passed, {k} skipped"),
            (0, 0, f) => format!("; {f} failed"),
            (p, 0, f) => format!("; {p} passed, {f} failed"),
            (0, k, f) => format!("; {k} skipped, {f} failed"),
            (p, k, f) => format!("; {p} passed, {k} skipped, {f} failed"),
        };
        budget.emit(&paint(
            &format!(
                "… {} noise lines dropped{tests} (--raw keeps them)",
                report.dropped
            ),
            Tone::Dim,
            out.color,
        ));
    }
    if report.explain {
        if let Some(profile) = &report.profile {
            budget.emit(&paint(
                &format!("… detected: {profile}"),
                Tone::Dim,
                out.color,
            ));
        }
        let kinds = report.stats.render();
        if !kinds.is_empty() {
            budget.emit(&paint(&format!("… by kind: {kinds}"), Tone::Dim, out.color));
        }
    }
    if report.grep_dropped > 0 {
        let why = if report.nothing_matched && report.dropped > 0 {
            "; nothing matched, and the noise filter dropped lines (--raw matches those too)"
        } else if report.nothing_matched {
            "; nothing matched"
        } else {
            ""
        };
        budget.emit(&paint(
            &format!(
                "… {} lines not matching --grep dropped{why}",
                report.grep_dropped
            ),
            Tone::Dim,
            out.color,
        ));
    }
    if let Some((log, n)) = &report.saved_log {
        let p = log.display();
        budget.emit(&paint(
            &format!("… full output ({n} lines): acus find 'PAT' {p}, or acus show {p}:A-B"),
            Tone::Dim,
            out.color,
        ));
    }
}

/// The code at each reference, one block per snippet with every referenced line marked.
fn print_snippets(out: &Out, report: &RunReport, budget: &mut Budget) {
    for snippet in &report.snippets {
        let ls: Vec<String> = snippet.marks.iter().map(ToString::to_string).collect();
        let word = if snippet.marks.len() > 1 {
            "lines"
        } else {
            "line"
        };
        budget.emit(&out.header(&format!("== {} ({word} {})", snippet.label, ls.join(", "))));
        for (i, t) in snippet.lines.iter().enumerate() {
            budget.emit(&out.numbered(
                snippet.start + i,
                &cut_line(t, report.cut),
                snippet.marks.contains(&(snippet.start + i)),
            ));
        }
    }
    if report.more_locations > 0 {
        budget.emit(&format!(
            "… {} more locations (--max-refs {})",
            report.more_locations,
            report.max_refs + report.more_locations
        ));
    }
}

/// A saved log last written under 10 s ago may still be growing; the note says so.
fn print_growing(out: &Out, report: &RunReport, budget: &mut Budget) {
    let (Some((path, _)), Some(age)) = (&report.input_log, report.growing) else {
        return;
    };
    let p = path.display();
    budget.emit(&paint(
        &format!(
            "{p} was written to {age}s ago and may still be growing; rerun acus run --log {p} for newer lines"
        ),
        Tone::Dim,
        out.color,
    ));
}
