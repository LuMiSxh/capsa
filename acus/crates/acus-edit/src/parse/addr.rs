// SPDX-License-Identifier: MPL-2.0
//! Parses the `PATH#Symbol` and `PATH:N` addresses of directives.

use super::{InsertAt, LineAnchor};
use crate::error::{EditError, Result};

pub(super) fn sym_addr(rest: &str, n: usize, directive: &str) -> Result<(String, String)> {
    match rest.trim().split_once('#') {
        Some((p, s)) if !p.is_empty() && !s.is_empty() => Ok((p.into(), s.into())),
        _ => Err(EditError::ExpectedSymbol {
            line: n,
            directive: directive.into(),
        }),
    }
}

/// Parses `PATH:A-B` (or `PATH:A` for one line) into the path and the 1-based inclusive range.
pub(super) fn line_range(rest: &str, n: usize, directive: &str) -> Result<(String, usize, usize)> {
    let bad = || EditError::BadLineRange {
        line: n,
        directive: directive.into(),
    };
    let (path, range) = rest.trim().rsplit_once(':').ok_or_else(bad)?;
    let (start, end) = match range.split_once('-') {
        Some((a, b)) => (a.parse::<usize>(), b.parse::<usize>()),
        None => (range.parse::<usize>(), range.parse::<usize>()),
    };
    match (start, end) {
        (Ok(a), Ok(b)) if !path.is_empty() && (1..=b).contains(&a) => Ok((path.into(), a, b)),
        _ => Err(bad()),
    }
}

/// Parses the `*** After:` of `*** Move Lines`: `PATH#Symbol`, or `PATH:N` with N 0 for the start
/// of the file.
pub(super) fn move_anchor(rest: &str, n: usize) -> Result<LineAnchor> {
    let rest = rest.trim();
    if rest.contains('#') {
        let (path, symbol) = sym_addr(rest, n, "After")?;
        return Ok(LineAnchor::Symbol { path, symbol });
    }
    match rest
        .rsplit_once(':')
        .and_then(|(p, l)| Some((p, l.parse::<usize>().ok()?)))
    {
        Some((p, after)) if !p.is_empty() => Ok(LineAnchor::Line {
            path: p.into(),
            after,
        }),
        _ => Err(EditError::ExpectedSymbolOrLine {
            line: n,
            directive: "After".into(),
        }),
    }
}

/// Parses `PATH#Symbol` or `PATH:N`.
pub(super) fn insert_addr(rest: &str, n: usize, directive: &str) -> Result<(String, InsertAt)> {
    let rest = rest.trim();
    if rest.contains('#') {
        let (p, s) = sym_addr(rest, n, directive)?;
        return Ok((p, InsertAt::Symbol(s)));
    }
    match rest
        .rsplit_once(':')
        .and_then(|(p, l)| Some((p, l.parse::<usize>().ok().filter(|&l| l > 0)?)))
    {
        Some((p, l)) if !p.is_empty() => Ok((p.into(), InsertAt::Line(l))),
        _ => Err(EditError::ExpectedSymbolOrLine {
            line: n,
            directive: directive.into(),
        }),
    }
}
