// SPDX-License-Identifier: MPL-2.0
//! The line-per-file views of `acus diff`: `--stat`, `--name-only` and `--conflicts`.

use super::report::{Changes, Conflicts};
use super::{Args, counts, file_path, status, total};
use crate::out::{Budget, Out};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::render::{Tone, paint};

/// Prints the header, one `path +A -R` line per file and the total.
pub(super) fn print_stat(a: &Args, out: &Out, c: &Changes) {
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    if let Some(h) = &c.header {
        budget.emit(&out.header(&h.header()));
    }
    for f in &c.files {
        let count = counts(total(&f.diff, true), total(&f.diff, false));
        let path = file_path(&f.diff);
        let line = if count.is_empty() {
            path
        } else {
            format!("{path} {count}")
        };
        budget.emit(&line);
    }
    let (add, del) = c.files.iter().fold((0, 0), |(x, y), f| {
        (x + total(&f.diff, true), y + total(&f.diff, false))
    });
    budget.emit(&total_line(c.files.len(), add, del));
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
}

/// Prints the header and one `STATUS path` line per changed file.
pub(super) fn print_name_only(a: &Args, out: &Out, c: &Changes) {
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    if let Some(h) = &c.header {
        budget.emit(&out.header(&h.header()));
    }
    for f in &c.files {
        budget.emit(&format!("{} {}", status(&f.diff), file_path(&f.diff)));
    }
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
}

/// Prints the unmerged paths with their conflict ranges, and a hint for resolving them.
pub(super) fn print_conflicts(out: &Out, c: &Conflicts) -> Outcome {
    if out.format == Format::Json {
        let rows: Vec<_> = c
            .unmerged
            .iter()
            .map(|u| {
                let ranges: Vec<_> = u
                    .ranges
                    .iter()
                    .map(|(s, e)| serde_json::json!([s, e]))
                    .collect();
                serde_json::json!({ "path": u.path, "ranges": ranges })
            })
            .collect();
        let mut root = serde_json::Map::new();
        if let Some(h) = &c.header {
            root.insert("header".into(), h.json());
        }
        root.insert("conflicts".into(), serde_json::Value::Array(rows));
        outln!("{}", serde_json::Value::Object(root));
    } else {
        if let Some(h) = &c.header {
            outln!("{}", out.header(&h.header()));
        }
        for u in &c.unmerged {
            outln!("{}", conflict_line(&u.path, &u.ranges));
        }
        if !c.unmerged.is_empty() {
            outln!(
                "{}",
                paint(
                    "hint: resolve in acus patch with *** Resolve Conflict: PATH --ours|--theirs",
                    Tone::Dim,
                    out.color,
                )
            );
        }
    }
    if c.unmerged.is_empty() {
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
