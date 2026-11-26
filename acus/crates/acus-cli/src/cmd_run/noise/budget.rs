// SPDX-License-Identifier: MPL-2.0
//! Keeps the last lines of a long output plus every failure block, so a failure in the middle
//! survives the line budget.

use super::super::narrow::FAILURE;

/// Returns the lines to print: every failure block, the last `n` lines, and a gap line where
/// lines were left out. Also returns how many non-blank lines were dropped.
pub(in crate::cmd_run) fn keep(
    lines: Vec<String>,
    n: usize,
    max_lines: usize,
) -> (Vec<String>, usize) {
    let len = lines.len();
    if len <= n {
        return (lines, 0);
    }
    let mut show = vec![false; len];
    show[len - n..].fill(true);
    let mut i = 0;
    while i < len {
        if !FAILURE.is_match(&lines[i]) {
            i += 1;
            continue;
        }
        let end = (i + 1..len)
            .find(|&j| lines[j].trim().is_empty())
            .unwrap_or(len)
            .min(i + max_lines.max(1));
        show[i..end].fill(true);
        i = end;
    }
    let mut out = Vec::new();
    let mut gap = 0;
    let mut dropped = 0;
    for (line, shown) in lines.into_iter().zip(show) {
        if shown {
            if gap > 0 {
                out.push(format!("… {gap} lines dropped (--max-output {n})"));
                gap = 0;
            }
            out.push(line);
        } else if !line.trim().is_empty() {
            gap += 1;
            dropped += 1;
        }
    }
    if gap > 0 {
        out.push(format!("… {gap} lines dropped (--max-output {n})"));
    }
    (out, dropped)
}
