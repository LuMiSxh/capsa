// SPDX-License-Identifier: MPL-2.0
//! Applies the line-addressed directives — `*** Resolve Conflict`, `*** Delete Lines` and
//! `*** Move Lines` — against the files as they were before the patch, so none shifts another.

use super::Change;
use super::conflict::{Resolution, resolutions};
use super::files::Files;
use super::symbols::symbol_lines;
use crate::error::{EditError, Result};
use crate::parse::{LineAnchor, Op};
use std::collections::BTreeMap;

/// One line directive: lines `start..=end` (1-based) of `path`, and where a move puts them.
struct Part<'a> {
    name: &'static str,
    line: usize,
    path: &'a str,
    start: usize,
    end: usize,
    expect: Option<&'a str>,
    moves: bool,
    to: Option<&'a LineAnchor>,
}

/// Where a move goes: the file and the 1-based line it goes after (0 for the file start).
type Target = Option<(String, usize)>;

fn part(op: &Op) -> Option<Part<'_>> {
    match op {
        Op::DeleteLines {
            path,
            start,
            end,
            expect,
            line,
        } => Some(Part {
            name: "Delete Lines",
            line: *line,
            path,
            start: *start,
            end: *end,
            expect: expect.as_deref(),
            moves: false,
            to: None,
        }),
        Op::MoveLines {
            path,
            start,
            end,
            expect,
            to,
            line,
        } => Some(Part {
            name: "Move Lines",
            line: *line,
            path,
            start: *start,
            end: *end,
            expect: expect.as_deref(),
            moves: true,
            to: to.as_ref(),
        }),
        _ => None,
    }
}

/// Applies every line-addressed directive of one patch. All of them count lines in the files as
/// they were before the patch, so they are collected against those lines first; the other
/// directives of the patch run after this.
pub(super) fn apply_line_edits(
    files: &mut Files,
    conflicts: &[&Op],
    line_ops: &[&Op],
) -> Result<Vec<Change>> {
    let resolved = resolutions(files, conflicts)?;
    apply_line_ranges(files, line_ops, &resolved)
}

/// Checks every line directive against the files as they were before the patch, then writes
/// each file the directives touch once.
fn apply_line_ranges(
    files: &mut Files,
    ops: &[&Op],
    resolved: &BTreeMap<String, Vec<Resolution>>,
) -> Result<Vec<Change>> {
    let parts: Vec<Part> = ops.iter().filter_map(|op| part(op)).collect();
    check_sources(files, &parts)?;
    check_overlaps(&parts)?;
    let targets = resolve_targets(files, &parts)?;
    check_targets(&parts, &targets)?;
    check_resolved(&parts, &targets, resolved)?;
    rebuild(files, &parts, &targets, resolved)
}

fn check_sources(files: &mut Files, parts: &[Part]) -> Result<()> {
    for p in parts {
        if p.start == 0 || p.start > p.end {
            return Err(EditError::BadLineRange {
                line: p.line,
                directive: p.name.into(),
            });
        }
        let lines = files.lines(p.path)?;
        if p.end > lines.len() {
            return Err(EditError::RangePastEnd {
                path: p.path.into(),
                start: p.start,
                end: p.end,
                len: lines.len(),
            });
        }
        let Some(expect) = p.expect else {
            return Err(EditError::LinesNoExpect {
                path: p.path.into(),
                start: p.start,
                end: p.end,
                directive: p.name,
            });
        };
        let actual = &lines[p.start - 1];
        if actual.trim_end() != expect.trim_end() {
            return Err(EditError::ExpectMismatch {
                path: p.path.into(),
                line: p.start,
                expected: expect.into(),
                actual: actual.clone(),
            });
        }
    }
    Ok(())
}

/// Refuses two directives that cut the same line of one file.
fn check_overlaps(parts: &[Part]) -> Result<()> {
    for (i, a) in parts.iter().enumerate() {
        for b in &parts[i + 1..] {
            if a.path == b.path && b.start <= a.end && a.start <= b.end {
                return Err(EditError::LinesOverlap {
                    path: a.path.into(),
                    first: format!("{}-{}", a.start, a.end),
                    second: format!("{}-{}", b.start, b.end),
                });
            }
        }
    }
    Ok(())
}

/// Resolves each move's `*** After:`, a symbol's end line included, against the original files.
fn resolve_targets(files: &mut Files, parts: &[Part]) -> Result<Vec<Target>> {
    let mut targets = Vec::with_capacity(parts.len());
    for p in parts {
        if !p.moves {
            targets.push(None);
            continue;
        }
        let Some(anchor) = p.to else {
            return Err(EditError::MoveLinesNoDestination {
                path: p.path.into(),
                start: p.start,
                end: p.end,
            });
        };
        let (path, after) = match anchor {
            LineAnchor::Line { path, after } => (path.clone(), *after),
            LineAnchor::Symbol { path, symbol } => {
                (path.clone(), symbol_lines(files, path, symbol)?.1)
            }
        };
        let len = files.lines(&path)?.len();
        if after > len {
            return Err(EditError::InsertPastEnd {
                path,
                line: after,
                len,
            });
        }
        targets.push(Some((path, after)));
    }
    Ok(targets)
}

/// Refuses a move whose `*** After:` line is one of the lines being moved out of its file.
fn check_targets(parts: &[Part], targets: &[Target]) -> Result<()> {
    for (path, after) in targets.iter().flatten() {
        for p in parts.iter().filter(|p| p.path == path) {
            if (p.start..=p.end).contains(after) {
                return Err(EditError::LineInsideMove {
                    path: path.clone(),
                    line: *after,
                    start: p.start,
                    end: p.end,
                });
            }
        }
    }
    Ok(())
}

/// Refuses a line directive that cuts, or moves lines into, a conflict section another directive
/// resolves: both would edit the same lines of the file before the patch.
fn check_resolved(
    parts: &[Part],
    targets: &[Target],
    resolved: &BTreeMap<String, Vec<Resolution>>,
) -> Result<()> {
    for (p, target) in parts.iter().zip(targets) {
        let cuts = resolved
            .get(p.path)
            .is_some_and(|rs| rs.iter().any(|r| p.start - 1 < r.end && r.start < p.end));
        if cuts {
            return Err(EditError::ResolveOverlap {
                path: p.path.into(),
                directive: p.name,
                line: p.line,
                place: format!("lines {}-{}", p.start, p.end),
            });
        }
        let Some((dest, after)) = target else {
            continue;
        };
        let inside = resolved
            .get(dest.as_str())
            .is_some_and(|rs| rs.iter().any(|r| r.start < *after && *after < r.end));
        if inside {
            return Err(EditError::ResolveOverlap {
                path: dest.clone(),
                directive: p.name,
                line: p.line,
                place: format!("the insert point after line {after}"),
            });
        }
    }
    Ok(())
}

/// Rebuilds each touched file from the lines before the patch: each resolved section is replaced,
/// the cut lines are dropped, and each move's lines are inserted after its target line, in the
/// order the directives appear.
fn rebuild(
    files: &mut Files,
    parts: &[Part],
    targets: &[Target],
    resolved: &BTreeMap<String, Vec<Resolution>>,
) -> Result<Vec<Change>> {
    let mut orig: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let touched = parts
        .iter()
        .map(|p| p.path)
        .chain(targets.iter().flatten().map(|(dest, _)| dest.as_str()))
        .chain(resolved.keys().map(String::as_str));
    for path in touched {
        if !orig.contains_key(path) {
            orig.insert(path.to_owned(), files.lines(path)?.clone());
        }
    }
    let mut cut: BTreeMap<&str, Vec<bool>> = orig
        .iter()
        .map(|(path, lines)| (path.as_str(), vec![false; lines.len()]))
        .collect();
    let mut inserts: BTreeMap<&str, BTreeMap<usize, Vec<String>>> = BTreeMap::new();
    for (p, target) in parts.iter().zip(targets) {
        if let Some(flags) = cut.get_mut(p.path) {
            flags[p.start - 1..p.end].fill(true);
        }
        if let Some((dest, after)) = target {
            let block = orig[p.path][p.start - 1..p.end].to_vec();
            inserts
                .entry(dest.as_str())
                .or_default()
                .entry(*after)
                .or_default()
                .extend(block);
        }
    }
    for (path, lines) in &orig {
        let flags = &cut[path.as_str()];
        let ins = inserts.get(path.as_str());
        let sections: &[Resolution] = resolved.get(path.as_str()).map_or(&[], Vec::as_slice);
        let mut out = Vec::with_capacity(lines.len());
        let mut edits = Vec::new();
        let mut i = 0;
        while i <= lines.len() {
            if let Some(block) = ins.and_then(|m| m.get(&i)) {
                edits.push((out.len(), block.len()));
                out.extend(block.iter().cloned());
            }
            if i == lines.len() {
                break;
            }
            if let Some(section) = sections.iter().find(|r| r.start == i) {
                edits.push((out.len(), section.lines.len()));
                out.extend(section.lines.iter().cloned());
                i = section.end;
                continue;
            }
            if !flags[i] {
                out.push(lines[i].clone());
            }
            i += 1;
        }
        let f = files.get(path)?;
        f.lines = Some(out);
        f.dirty = true;
        f.edits = edits;
    }
    Ok(changes(parts, targets, resolved))
}

/// One summary per directive and per resolved file: the lines it removes and, for a move, the
/// lines it adds at its target. `settle` recounts them from the file texts, so the numbers only
/// name the paths.
fn changes(
    parts: &[Part],
    targets: &[Target],
    resolved: &BTreeMap<String, Vec<Resolution>>,
) -> Vec<Change> {
    let mut out = Vec::new();
    for (p, target) in parts.iter().zip(targets) {
        let n = p.end - p.start + 1;
        match target {
            Some((dest, _)) if dest == p.path => out.push(Change::Modified {
                path: p.path.into(),
                added: n,
                removed: n,
            }),
            Some((dest, _)) => {
                out.push(Change::Modified {
                    path: p.path.into(),
                    added: 0,
                    removed: n,
                });
                out.push(Change::Modified {
                    path: dest.clone(),
                    added: n,
                    removed: 0,
                });
            }
            None => out.push(Change::Modified {
                path: p.path.into(),
                added: 0,
                removed: n,
            }),
        }
    }
    for path in resolved.keys() {
        out.push(Change::Modified {
            path: path.clone(),
            added: 0,
            removed: 0,
        });
    }
    out
}
