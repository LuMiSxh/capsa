// SPDX-License-Identifier: MPL-2.0
//! Prints `acus patch` results: the summary, the notes, the written lines and any diffs.

use super::{PatchReport, WrittenRegion};
use crate::out::{Out, more_lines};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;

/// Written lines shown per region; longer regions end with a `show` hint.
const REGION_LINES: usize = 20;
/// Region lines echoed per patch, so a large patch does not flood the output.
const ECHO_LINES: usize = 120;
/// Regions past the echo cap that are named by address, the rest are counted.
const NAMED_REGIONS: usize = 10;
/// Lines kept at each end of a long `Replace Symbol` body.
const SYMBOL_EDGE_LINES: usize = 5;

/// Prints the report in the format the run asked for and returns the patch's outcome.
pub(crate) fn print(report: &PatchReport, out: &Out) -> Outcome {
    if report.diff {
        for diff in &report.diffs {
            outprint!("{diff}");
        }
        for note in &report.notes {
            outln!("{note}");
        }
        return Outcome::Found;
    }
    if out.format == Format::Json {
        return print_json(report);
    }
    for change in &report.changes {
        outln!("{change}");
    }
    for note in &report.notes {
        outln!("{note}");
    }
    if !report.quiet {
        echo_regions(out, report);
    }
    if report.check {
        outln!("(check only, nothing written)");
    }
    Outcome::Found
}

/// Prints the `--json` document and returns the patch's outcome.
fn print_json(report: &PatchReport) -> Outcome {
    outln!("{}", json(report));
    Outcome::Found
}

/// The `--json` document: the changes, the notes and the written regions.
pub(crate) fn json(report: &PatchReport) -> serde_json::Value {
    let regions: Vec<_> = report
        .regions
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
    serde_json::json!({
        "changes": &report.changes,
        "notes": &report.notes,
        "regions": regions,
        "written": report.written
    })
}

/// Echoes the written regions up to the line cap; the rest are named as `acus show` addresses.
fn echo_regions(out: &Out, report: &PatchReport) {
    let mut echoed = 0;
    let mut unshown: Vec<String> = Vec::new();
    for r in &report.regions {
        let end = r.start + r.lines.len() - 1;
        let shown = if r.long {
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
        for line in region_lines(out, r) {
            outln!("{line}");
        }
    }
    if !unshown.is_empty() {
        outln!("{}", more_regions(&unshown));
    }
}

/// The printed lines of a region: all of it, or the edges of a long `Replace Symbol` body.
fn region_lines(out: &Out, r: &WrittenRegion) -> Vec<String> {
    let n = r.lines.len();
    let end = r.start + n - 1;
    let numbered = |i: usize| out.numbered(r.start + i, &r.lines[i], false);
    if r.long {
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
