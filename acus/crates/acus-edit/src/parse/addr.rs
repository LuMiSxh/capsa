// SPDX-License-Identifier: MPL-2.0
//! Parses the `PATH#Symbol` and `PATH:N` addresses of directives.

use super::InsertAt;
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
