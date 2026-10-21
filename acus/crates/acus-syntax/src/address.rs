// SPDX-License-Identifier: MPL-2.0
//! Parses and resolves `path`, `path:A-B` and `path#Symbol` references. Symbol lookup
//! tries the exact qualified name first, so `Class.method` and `Class::method` both work
//! in every language.
use crate::{Outline, Symbol};
use std::fmt;

/// Names what an address points at. Lines are 1-based and inclusive, matching how `path:A-B` is
/// typed and printed. Symbols need an outline, so line ranges are the fallback for files without
/// a grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Whole,
    /// Holds a 1-based inclusive line range.
    Lines(usize, usize),
    /// Names line N to the end of the file, `path:N-`. Resolves at show time, like `Tail`.
    From(usize),
    /// Names the last N lines, `path:-N`. Needs the line count, so it resolves at show time.
    Tail(usize),
    Symbol(String),
}

/// Holds one `path`, `path:A-B` or `path#Symbol` reference from the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub path: String,
    pub target: Target,
}

impl Address {
    /// Parses `path#Sym`, `path:START-END`, `path:LINE`, `path:N-`, `path:-N` or `path`. A `:`
    /// suffix counts only when it is numeric, so Windows drive letters survive.
    pub fn parse(s: &str) -> Address {
        if let Some((path, sym)) = s.split_once('#') {
            return Address {
                path: path.into(),
                target: Target::Symbol(sym.into()),
            };
        }
        if let Some((path, range)) = s.rsplit_once(':') {
            // `-N` is the last N lines, as `tail -n N` counts them.
            if let Some(n) = range.strip_prefix('-').and_then(|n| n.parse().ok()) {
                return Address {
                    path: path.into(),
                    target: Target::Tail(n),
                };
            }
            // `N-` is line N to the end, as `sed -n 'N,$p'` reads it.
            if let Some(n) = range.strip_suffix('-').and_then(|n| n.parse().ok()) {
                return Address {
                    path: path.into(),
                    target: Target::From(n),
                };
            }
            // `A,B` is the sed-style spelling of `A-B`.
            let (a, b) = range.split_once(['-', ',']).unwrap_or((range, range));
            if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
                return Address {
                    path: path.into(),
                    target: Target::Lines(a, b),
                };
            }
        }
        Address {
            path: s.into(),
            target: Target::Whole,
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.target {
            Target::Whole => write!(f, "{}", self.path),
            Target::Lines(a, b) if a == b => write!(f, "{}:{a}", self.path),
            Target::Lines(a, b) => write!(f, "{}:{a}-{b}", self.path),
            Target::From(n) => write!(f, "{}:{n}-", self.path),
            Target::Tail(n) => write!(f, "{}:-{n}", self.path),
            Target::Symbol(s) => write!(f, "{}#{s}", self.path),
        }
    }
}

/// Holds the outcome of a symbol lookup. Every match is returned rather than the first, so a
/// same-named pair is never silently picked for an edit.
pub enum Resolve<'a> {
    Found(&'a Symbol),
    Ambiguous(Vec<&'a Symbol>),
    Missing,
}

/// Looks up the exact qualified name first, then a unique `::`-suffix or plain name.
pub fn resolve<'a>(o: &'a Outline, query: &str) -> Resolve<'a> {
    match resolve_as(o, query) {
        // `Class.method` as written in Python, TS or Swift; keys like `a.b` match first.
        Resolve::Missing if query.contains('.') => resolve_as(o, &query.replace('.', "::")),
        r => r,
    }
}

fn resolve_as<'a>(o: &'a Outline, query: &str) -> Resolve<'a> {
    let suffix = format!("::{query}");
    let exact: Vec<&Symbol> = o.symbols.iter().filter(|s| s.qual == query).collect();
    let hits = if exact.is_empty() {
        o.symbols
            .iter()
            .filter(|s| s.qual.ends_with(&suffix) || s.name == query)
            .collect()
    } else {
        exact
    };
    // A type and its impl/extension share a qualified name; the one with children wins.
    let has_children = |s: &Symbol| {
        let prefix = format!("{}::", s.qual);
        o.symbols.iter().any(|c| {
            c.qual.starts_with(&prefix) && s.start_line <= c.start_line && c.end_line <= s.end_line
        })
    };
    match hits.as_slice() {
        [] => Resolve::Missing,
        [s] => Resolve::Found(s),
        // Same-kind twins without children (two `## Usage` headings) stay ambiguous.
        [first, ..] if hits.iter().all(|s| s.qual == first.qual) => {
            match hits.iter().find(|s| has_children(s)) {
                Some(s) => Resolve::Found(s),
                None if hits.iter().all(|s| s.kind == first.kind) => Resolve::Ambiguous(hits),
                None => Resolve::Found(first),
            }
        }
        _ => Resolve::Ambiguous(hits),
    }
}
