// SPDX-License-Identifier: MPL-2.0
//! Reads the unmerged paths of `acus diff --conflicts` and the lines of their conflict markers.

use super::report::{Conflict, Conflicts};
use super::status::Status;
use anyhow::Result;

/// Reads the unmerged paths, limited to `paths`, with the line ranges of their markers.
pub(super) fn compute(paths: &[String], rev: Option<&str>) -> Result<Conflicts> {
    let status = Status::load(paths)?;
    let unmerged: Vec<Conflict> = status
        .unmerged
        .iter()
        .map(|p| Conflict {
            path: p.clone(),
            ranges: sections(p),
        })
        .collect();
    Ok(Conflicts {
        header: rev.is_none().then_some(status),
        unmerged,
    })
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
