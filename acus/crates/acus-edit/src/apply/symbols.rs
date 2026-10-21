// SPDX-License-Identifier: MPL-2.0
//! Symbol edits: finding a symbol's lines, moving it between files and re-indenting it.

use super::Change;
use super::files::Files;
use crate::error::{EditError, Result};
use crate::parse::Dest;
use acus_syntax::{Lang, Resolve, attached_start, outline, resolve};
use std::path::Path;

/// 0-based, end-exclusive line range of `path#symbol`. A name that a type and its impl (or any
/// other symbols with the same qualified name) share is refused: `resolve` would pick the impl,
/// and an edit must not silently pick one.
pub(super) fn symbol_lines(files: &mut Files, path: &str, symbol: &str) -> Result<(usize, usize)> {
    let src = files.lines(path)?.join("\n");
    let Some(o) = Lang::from_path(Path::new(path)).and_then(|l| outline(l, &src)) else {
        return Err(EditError::NoSyntax { path: path.into() });
    };
    match resolve(&o, symbol) {
        Resolve::Found(s) => {
            let twins: Vec<_> = o.symbols.iter().filter(|t| t.qual == s.qual).collect();
            if twins.len() > 1 {
                let c: Vec<_> = twins
                    .iter()
                    .map(|t| format!("{} {} ({}-{})", t.kind, t.name, t.start_line, t.end_line))
                    .collect();
                return Err(EditError::AmbiguousSymbol {
                    path: path.into(),
                    symbol: symbol.into(),
                    candidates: c,
                });
            }
            Ok((s.start_line - 1, s.end_line))
        }
        Resolve::Missing => Err(EditError::NoSymbol {
            path: path.into(),
            symbol: symbol.into(),
        }),
        Resolve::Ambiguous(v) => {
            let c: Vec<_> = v
                .iter()
                .map(|s| format!("{path}#{} ({}-{})", s.qual, s.start_line, s.end_line))
                .collect();
            Err(EditError::AmbiguousName {
                path: path.into(),
                symbol: symbol.into(),
                candidates: c,
            })
        }
    }
}

/// Grows a symbol's range upwards over the doc comments, attributes and decorators that
/// belong to it.
pub(super) fn with_attached(
    lines: &[String],
    path: &str,
    start: usize,
    end: usize,
) -> (usize, usize) {
    (attached_start(lines, path, start), end)
}

/// Adds one blank line next to a removed range so no double gap remains.
pub(super) fn with_blank(lines: &[String], start: usize, end: usize) -> (usize, usize) {
    let blank = |i: usize| lines.get(i).is_some_and(|l| l.trim().is_empty());
    if blank(end) && (start == 0 || blank(start - 1) || end + 1 < lines.len()) {
        (start, end + 1)
    } else if start > 0 && blank(start - 1) {
        (start - 1, end)
    } else {
        (start, end)
    }
}

/// Languages a symbol may move between; TS, JS and Svelte scripts mix freely.
fn family(path: &str) -> Option<&'static str> {
    Some(match Lang::from_path(Path::new(path))? {
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript | Lang::Svelte => "js",
        l => l.name(),
    })
}

pub(super) fn move_symbol(
    files: &mut Files,
    path: &str,
    symbol: &str,
    to: &Dest,
) -> Result<Vec<Change>> {
    let dest = match to {
        Dest::Before { path, .. } | Dest::After { path, .. } | Dest::End { path } => path,
    };
    if let (Some(a), Some(b)) = (family(path), family(dest))
        && a != b
    {
        return Err(EditError::MoveAcrossLanguages {
            path: path.into(),
            symbol: symbol.into(),
            from: a,
            dest: dest.into(),
            to: b,
        });
    }
    let (start, end) = symbol_lines(files, path, symbol)?;
    if let Dest::Before { path: p, symbol: s } | Dest::After { path: p, symbol: s } = to
        && p == path
    {
        let (ds, _) = symbol_lines(files, p, s)?;
        if (start..end).contains(&ds) {
            return Err(EditError::MoveIntoItself {
                path: path.into(),
                symbol: symbol.into(),
            });
        }
    }
    let (start, end) = with_attached(files.lines(path)?, path, start, end);
    let lines = files.lines(path)?;
    let block = lines[start..end].to_vec();
    let (s, e) = with_blank(lines, start, end);
    let f = files.get(path)?;
    f.splice(s, e - s, &[]);
    f.dirty = true;
    let mut created = false;
    let at = match to {
        Dest::Before { path, symbol } => {
            let (s, e) = symbol_lines(files, path, symbol)?;
            let lines = files.lines(path)?;
            let mut ins = reindent(&block, indent(&lines[s]));
            ins.push(String::new());
            (with_attached(lines, path, s, e).0, ins)
        }
        Dest::After { path, symbol } => {
            let (s, e) = symbol_lines(files, path, symbol)?;
            let ins = reindent(&block, indent(&files.lines(path)?[s]));
            (e, [vec![String::new()], ins].concat())
        }
        Dest::End { path } => {
            let f = files.get(path)?;
            let lines = f.lines.get_or_insert_with(|| {
                created = true;
                vec![]
            });
            let gap = lines.last().is_some_and(|l| !l.trim().is_empty());
            let block = reindent(&block, "");
            let ins = if gap {
                [vec![String::new()], block].concat()
            } else {
                block
            };
            (lines.len(), ins)
        }
    };
    let (at, ins) = at;
    let f = files.get(dest)?;
    f.splice(at, 0, &ins);
    f.dirty = true;
    Ok(if dest == path {
        vec![Change::Modified {
            path: path.into(),
            added: block.len(),
            removed: block.len(),
        }]
    } else {
        vec![
            Change::Modified {
                path: path.into(),
                added: 0,
                removed: block.len(),
            },
            if created {
                Change::Added {
                    path: dest.clone(),
                    lines: ins.len(),
                }
            } else {
                Change::Modified {
                    path: dest.clone(),
                    added: block.len(),
                    removed: 0,
                }
            },
        ]
    })
}

fn indent(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Shifts a moved block so its least indented line gets `to`, e.g. a method moved out of a
/// class.
fn reindent(block: &[String], to: &str) -> Vec<String> {
    let from = block
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent(l))
        .min_by_key(|i| i.len())
        .unwrap_or("");
    block
        .iter()
        .map(|l| match l.strip_prefix(from) {
            _ if l.trim().is_empty() => String::new(),
            Some(rest) => format!("{to}{rest}"),
            None => l.clone(),
        })
        .collect()
}
