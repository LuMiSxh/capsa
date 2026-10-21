// SPDX-License-Identifier: MPL-2.0
//! Applies each op to the in-memory files: one function per directive.

use super::Change;
use super::files::{File, Files};
use super::replace_all::replace_all;
use super::search::{Fix, search_block};
use super::symbols::{move_symbol, symbol_lines, with_attached, with_blank};
use crate::error::{EditError, Result};
use crate::parse::InsertAt;
use crate::{Chunk, Op};

pub(super) fn apply_op(files: &mut Files, op: &Op) -> Result<Vec<Change>> {
    Ok(match op {
        Op::Add { path, lines } => vec![add(files, path, lines)?],
        Op::Delete { path } => vec![delete(files, path)?],
        Op::Update {
            path,
            move_to,
            chunks,
        } => {
            vec![update(files, path, move_to.as_deref(), chunks)?]
        }
        Op::ReplaceSymbol {
            path,
            symbol,
            lines: body,
        } => {
            vec![replace_symbol(files, path, symbol, body)?]
        }
        Op::DeleteSymbol { path, symbol } => vec![delete_symbol(files, path, symbol)?],
        Op::MoveSymbol { path, symbol, to } => move_symbol(
            files,
            path,
            symbol,
            to.as_ref()
                .ok_or_else(|| EditError::MoveWithoutDestination {
                    symbol: symbol.into(),
                })?,
        )?,
        Op::ReplaceAll { paths, blocks } => replace_all(files, paths, blocks)?,
        Op::Insert {
            path,
            at,
            after,
            lines: body,
        } => {
            vec![insert(files, path, at, *after, body)?]
        }
        Op::Copy { from, to } => {
            vec![copy(
                files,
                from,
                to.as_deref()
                    .ok_or_else(|| EditError::CopyWithoutDestination { from: from.into() })?,
            )?]
        }
    })
}

fn add(files: &mut Files, path: &str, lines: &[String]) -> Result<Change> {
    let f = files.get(path)?;
    if f.lines.is_some() {
        return Err(EditError::AlreadyExists { path: path.into() });
    }
    f.lines = Some(lines.to_vec());
    f.dirty = true;
    Ok(Change::Added {
        path: path.to_owned(),
        lines: lines.len(),
    })
}

fn delete(files: &mut Files, path: &str) -> Result<Change> {
    let n = files.lines(path)?.len();
    let f = files.get(path)?;
    f.lines = None;
    f.dirty = true;
    Ok(Change::Deleted {
        path: path.to_owned(),
        lines: n,
    })
}

fn update(
    files: &mut Files,
    path: &str,
    move_to: Option<&str>,
    chunks: &[Chunk],
) -> Result<Change> {
    files.lines(path)?;
    let f = files.get(path)?;
    let (mut numbered, mut trailing, mut indented, mut skipped, mut crlf) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut added, mut removed) = (0, 0);
    for (i, c) in chunks.iter().enumerate() {
        let (edit, fix) = search_block(
            f.lines
                .as_ref()
                .ok_or_else(|| EditError::NotLoaded { path: path.into() })?,
            c,
            path,
            i + 1,
        )?;
        if c.old == c.new {
            skipped.push(c.line);
            continue;
        }
        let name = format!("{path} block {}", i + 1);
        if f.crlf && c.old.len() > 1 {
            crlf.push(name.clone());
        }
        match fix {
            Some(Fix::Numbers) => numbered.push(name),
            Some(Fix::Trailing) => trailing.push(name),
            Some(Fix::Indent) => indented.push(name),
            None => {}
        }
        added += edit.added;
        removed += edit.removed;
        f.splice(edit.at, edit.old, &edit.new);
    }
    files.numbered.extend(numbered);
    files.trailing.extend(trailing);
    files.indented.extend(indented);
    files.skipped.extend(skipped);
    files.crlf.extend(crlf);
    files.get(path)?.dirty = true;
    let Some(to) = move_to else {
        return Ok(Change::Modified {
            path: path.to_owned(),
            added,
            removed,
        });
    };
    let src = files.get(path)?;
    let contents = src.take_contents(false);
    let dst = files.get(to)?;
    if dst.lines.is_some() {
        return Err(EditError::Overwrite {
            to: to.into(),
            action: "move",
            from: path.into(),
        });
    }
    *dst = File {
        existed: dst.existed,
        ..contents
    };
    Ok(Change::Moved {
        from: path.to_owned(),
        to: to.to_owned(),
        added,
        removed,
    })
}

fn replace_symbol(files: &mut Files, path: &str, symbol: &str, body: &[String]) -> Result<Change> {
    if body.is_empty() {
        return Err(EditError::ReplaceNeedsLines {
            path: path.into(),
            symbol: symbol.into(),
        });
    }
    let (start, end) = symbol_lines(files, path, symbol)?;
    let f = files.get(path)?;
    f.splice(start, end - start, body);
    f.dirty = true;
    Ok(Change::Modified {
        path: path.to_owned(),
        added: body.len(),
        removed: end - start,
    })
}

fn delete_symbol(files: &mut Files, path: &str, symbol: &str) -> Result<Change> {
    let (start, end) = symbol_lines(files, path, symbol)?;
    let lines = files.lines(path)?;
    let (start, end) = with_attached(lines, path, start, end);
    let (s, e) = with_blank(lines, start, end);
    let f = files.get(path)?;
    f.splice(s, e - s, &[]);
    f.dirty = true;
    Ok(Change::Modified {
        path: path.to_owned(),
        added: 0,
        removed: end - start,
    })
}

fn insert(
    files: &mut Files,
    path: &str,
    at: &InsertAt,
    after: bool,
    body: &[String],
) -> Result<Change> {
    let at_line = match at {
        InsertAt::Symbol(s) => {
            let (start, end) = symbol_lines(files, path, s)?;
            if after {
                end
            } else {
                with_attached(files.lines(path)?, path, start, end).0
            }
        }
        InsertAt::Line(n) => {
            let len = files.lines(path)?.len();
            if *n > len {
                return Err(EditError::InsertPastEnd {
                    path: path.into(),
                    line: *n,
                    len,
                });
            }
            if after { *n } else { n - 1 }
        }
    };
    let f = files.get(path)?;
    f.splice(at_line, 0, body);
    f.dirty = true;
    Ok(Change::Modified {
        path: path.to_owned(),
        added: body.len(),
        removed: 0,
    })
}

fn copy(files: &mut Files, from: &str, to: &str) -> Result<Change> {
    let src = files.get(from)?;
    let Some(n) = src.lines.as_ref().map(Vec::len) else {
        return Err(EditError::NotFound { path: from.into() });
    };
    let contents = src.take_contents(true);
    let dst = files.get(to)?;
    if dst.lines.is_some() {
        return Err(EditError::Overwrite {
            to: to.into(),
            action: "copy",
            from: from.into(),
        });
    }
    *dst = File {
        existed: dst.existed,
        ..contents
    };
    Ok(Change::Added {
        path: to.to_owned(),
        lines: n,
    })
}
