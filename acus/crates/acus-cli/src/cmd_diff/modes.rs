// SPDX-License-Identifier: MPL-2.0
//! The line-per-file views of `acus diff`: `--stat`, `--name-only` and `--conflicts`.

use super::status::Status;
use super::{Args, FileDiff, counts, file_path, status, total};
use crate::fmt::{Budget, Out};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;

/// Prints the header, one `path +A -R` line per file and the total.
pub(super) fn print_stat(a: &Args, out: &Out, header: Option<&Status>, files: &[FileDiff]) {
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    if let Some(h) = header {
        budget.emit(&out.header(&h.header()));
    }
    for f in files {
        let c = counts(total(f, true), total(f, false));
        let path = file_path(f);
        let line = if c.is_empty() {
            path
        } else {
            format!("{path} {c}")
        };
        budget.emit(&line);
    }
    let (add, del) = files.iter().fold((0, 0), |(x, y), f| {
        (x + total(f, true), y + total(f, false))
    });
    budget.emit(&total_line(files.len(), add, del));
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
}

/// Prints the header and one `STATUS path` line per changed file.
pub(super) fn print_name_only(a: &Args, out: &Out, header: Option<&Status>, files: &[FileDiff]) {
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    if let Some(h) = header {
        budget.emit(&out.header(&h.header()));
    }
    for f in files {
        budget.emit(&format!("{} {}", status(f), file_path(f)));
    }
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
}

/// Prints the unmerged paths with their conflict ranges, and a hint for resolving them.
pub(super) fn conflicts(out: &Out, header: Option<&Status>, st: &Status) -> Outcome {
    if out.format == Format::Json {
        let rows: Vec<_> = st
            .unmerged
            .iter()
            .map(|p| {
                let ranges: Vec<_> = sections(p)
                    .iter()
                    .map(|(s, e)| serde_json::json!([s, e]))
                    .collect();
                serde_json::json!({ "path": p, "ranges": ranges })
            })
            .collect();
        let mut root = serde_json::Map::new();
        if let Some(h) = header {
            root.insert("header".into(), h.json());
        }
        root.insert("conflicts".into(), serde_json::Value::Array(rows));
        outln!("{}", serde_json::Value::Object(root));
    } else {
        if let Some(h) = header {
            outln!("{}", out.header(&h.header()));
        }
        for p in &st.unmerged {
            outln!("{}", conflict_line(p, &sections(p)));
        }
        if !st.unmerged.is_empty() {
            outln!(
                "{}",
                out.dim(
                    "hint: resolve in acus patch with *** Resolve Conflict: PATH --ours|--theirs"
                )
            );
        }
    }
    if st.unmerged.is_empty() {
        Outcome::Empty
    } else {
        Outcome::Found
    }
}

pub(super) fn total_line(files: usize, add: usize, del: usize) -> String {
    let c = counts(add, del);
    let s = if files == 1 { "" } else { "s" };
    if c.is_empty() {
        format!("== {files} file{s}")
    } else {
        format!("== {files} file{s} {c}")
    }
}

fn conflict_line(path: &str, ranges: &[(usize, usize)]) -> String {
    if ranges.is_empty() {
        return path.to_owned();
    }
    let list: Vec<_> = ranges.iter().map(|(s, e)| format!("{s}-{e}")).collect();
    format!("{path}: {}", list.join(", "))
}

/// The marker pairs of a file, as (start, end) line numbers.
fn sections(path: &str) -> Vec<(usize, usize)> {
    let Ok(src) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut start = None;
    for (i, line) in src.lines().enumerate() {
        let n = i + 1;
        if marker(line, "<<<<<<<") {
            start = Some(n);
        } else if marker(line, ">>>>>>>")
            && let Some(s) = start.take()
        {
            out.push((s, n));
        }
    }
    out
}

fn marker(line: &str, kind: &str) -> bool {
    line.strip_prefix(kind)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
}
