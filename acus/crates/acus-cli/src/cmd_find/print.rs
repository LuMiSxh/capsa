// SPDX-License-Identifier: MPL-2.0
//! Prints `acus find` results: hit lines, whole blocks, matched parts and JSON.

use super::matched;
use crate::fmt::{Budget, Out, clip, hit_text, more_lines};
use acus_search::{FileHits, Hit, Row};
use acus_syntax::Symbol;
use std::collections::{BTreeSet, HashMap};

fn sym(f: &FileHits, line: usize) -> Option<&Symbol> {
    f.outline.as_ref()?.enclosing(line)
}

/// The symbol label printed before each hit, in the order of `hits`. A label is left out when
/// every hit under it is on the symbol's first line: it would only repeat the hit's own line.
fn label_before<'a>(f: &'a FileHits, hits: &[&Hit]) -> Vec<Option<&'a Symbol>> {
    let mut out = vec![None; hits.len()];
    let mut i = 0;
    while i < hits.len() {
        let Some(s) = sym(f, hits[i].line) else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        while j < hits.len() && sym(f, hits[j].line).is_some_and(|t| t.qual == s.qual) {
            j += 1;
        }
        if hits[i..j].iter().any(|h| h.line != s.start_line) {
            out[i] = Some(s);
        }
        i = j;
    }
    out
}

/// One row per hit. With `context` = (before, after), the lines around the hits are printed
/// too, as grep prints them.
pub(super) fn print_lines(
    shown: &[(&FileHits, usize)],
    out: &Out,
    context: Option<(usize, usize)>,
    cut: usize,
    budget: &mut Budget,
) {
    for (f, n) in shown {
        budget.emit(&out.header(&f.path));
        let rows = match context {
            Some((before, after)) => f.context_rows(*n, before, after),
            None => f.hits[..*n].iter().map(Row::Hit).collect(),
        };
        let hits: Vec<&Hit> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Hit(h) => Some(*h),
                _ => None,
            })
            .collect();
        let mut labels = label_before(f, &hits).into_iter();
        for row in rows {
            match row {
                Row::Hit(h) => {
                    if let Some(s) = labels.next().flatten() {
                        budget.emit(
                            &out.dim(&format!("@{} {}-{}", s.qual, s.start_line, s.end_line)),
                        );
                    }
                    for (i, text) in h.text.split('\n').enumerate() {
                        let col = if i == 0 { h.col } else { 0 };
                        budget.emit(&out.numbered(h.line + i, &hit_text(text, col, cut), false));
                    }
                }
                Row::Context { line, text } => {
                    budget.emit(&out.context_line(line, &hit_text(text, 0, cut)));
                }
                Row::Break => budget.emit("--"),
            }
        }
    }
}

pub(super) fn print_blocks(
    shown: &[(&FileHits, usize)],
    out: &Out,
    max_lines: usize,
    cut: usize,
    budget: &mut Budget,
) {
    for (f, n) in shown {
        let lines: Vec<&str> = f.source.lines().collect();
        let hit_lines: BTreeSet<usize> = f.hits[..*n].iter().map(|h| h.line).collect();
        // First hit on a line wins, as the linear search it replaces did.
        let mut cols = HashMap::new();
        for h in &f.hits {
            cols.entry(h.line).or_insert(h.col);
        }
        let col = |i: usize| cols.get(&i).copied().unwrap_or(0);
        let mut done = BTreeSet::new();
        for &l in &hit_lines {
            let (title, a, b) = match sym(f, l) {
                Some(s) => (
                    format!("== {}#{} {}-{}", f.path, s.qual, s.start_line, s.end_line),
                    s.start_line,
                    s.end_line,
                ),
                // No symbol: the hit with two lines of context.
                None => (
                    format!("== {}:{l}", f.path),
                    l.saturating_sub(2).max(1),
                    (l + 2).min(lines.len()),
                ),
            };
            if !done.insert((a, b)) {
                continue;
            }
            budget.emit(&out.header(&title));
            let end = b.min(a + max_lines.max(1) - 1);
            for i in a..=end {
                let line = lines.get(i - 1).copied().unwrap_or("");
                let line = clip(line, col(i), cut);
                budget.emit(&out.numbered(i, &line, hit_lines.contains(&i)));
            }
            if end < b {
                budget.emit(&more_lines(&f.path, end + 1, b, b - end));
            }
        }
    }
}

/// The matched text alone: one row per match, symbol headers as in `print_lines`.
pub(super) fn print_only(shown: &[(&FileHits, usize)], out: &Out, cut: usize, budget: &mut Budget) {
    for (f, n) in shown {
        budget.emit(&out.header(&f.path));
        let hits: Vec<&Hit> = f.hits[..*n].iter().collect();
        let mut labels = label_before(f, &hits).into_iter();
        for h in &f.hits[..*n] {
            if let Some(s) = labels.next().flatten() {
                budget.emit(&out.dim(&format!("@{} {}-{}", s.qual, s.start_line, s.end_line)));
            }
            if h.spans.is_empty() {
                // -v has no match text; the line itself is the hit.
                budget.emit(&out.numbered(h.line, &hit_text(&h.text, h.col, cut), false));
            }
            for &(a, b) in &h.spans {
                let Some(t) = h.text.get(a..b) else {
                    continue;
                };
                let first = h.line + h.text.get(..a).unwrap_or_default().matches('\n').count();
                for (i, l) in t.split('\n').enumerate() {
                    budget.emit(&out.numbered(
                        first + i,
                        &clip(l.trim_end_matches('\r'), 0, cut),
                        false,
                    ));
                }
            }
        }
    }
}

pub(super) fn print_json(shown: &[(&FileHits, usize)], rest: usize, only: bool) {
    let files: Vec<_> = shown
        .iter()
        .map(|(f, n)| {
            let hits: Vec<_> = f.hits[..*n]
                .iter()
                .map(|h| {
                    let symbol = sym(f, h.line).map(|s| &s.qual);
                    if only && !h.spans.is_empty() {
                        let m: Vec<_> = matched(h).collect();
                        serde_json::json!({ "line": h.line, "matches": m, "symbol": symbol })
                    } else {
                        serde_json::json!({ "line": h.line, "text": h.text, "symbol": symbol })
                    }
                })
                .collect();
            serde_json::json!({ "path": f.path, "lang": f.lang, "hits": hits })
        })
        .collect();
    outln!(
        "{}",
        serde_json::json!({ "files": files, "truncated": rest })
    );
}
