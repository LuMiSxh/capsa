// SPDX-License-Identifier: MPL-2.0
//! Groups the changed lines of each file by enclosing symbol and outlines the added files.

use super::moves::{Move, MoveIndex, find_moves};
use super::report::{Group, NewFile, OutlineRow, TOP};
use super::{Args, FileDiff};
use crate::git::git;
use acus_syntax::{Lang, Outline, outline};
use std::path::Path;

/// Lines printed from a new file that has no outline.
const FIRST_LINES: usize = 20;

/// The derived view of one changed file, before it is paired with its diff.
#[derive(Default)]
pub(super) struct Derived {
    pub(super) symbols: Vec<Group>,
    pub(super) text_groups: Vec<Group>,
    pub(super) new_file: Option<NewFile>,
}

/// A changed file with the sources and outlines the groups need beyond its diff lines.
pub(super) struct View<'a> {
    pub(super) f: &'a FileDiff,
    pub(super) old_src: Option<String>,
    pub(super) new_src: Option<String>,
    pub(super) old_o: Option<Outline>,
    pub(super) new_o: Option<Outline>,
    /// The new-side outline as `acus outline` prints it, without attached comments.
    pub(super) display_o: Option<Outline>,
}

/// The moves of the compact view and the derived view of each file, in the order of `files`.
pub(super) fn derive(
    a: &Args,
    files: &[FileDiff],
    rev: Option<&str>,
    old_rev: &str,
    new_rev: Option<&str>,
) -> (Vec<Move>, Vec<Derived>) {
    let views = views(a, files, rev, old_rev, new_rev);
    let compact = a.lines && !a.full;
    let moves = if compact {
        find_moves(&views)
    } else {
        Vec::new()
    };
    let index = MoveIndex::new(&moves);
    let derived = views
        .iter()
        .enumerate()
        .map(|(i, v)| derive_file(v, i, compact, &index))
        .collect();
    (moves, derived)
}

/// Each changed file with the sources and outlines the groups need.
fn views<'a>(
    a: &Args,
    files: &'a [FileDiff],
    rev: Option<&str>,
    old_rev: &str,
    new_rev: Option<&str>,
) -> Vec<View<'a>> {
    // An empty revision reads the index.
    let show = |rev: &str, p: &str| git(&["show", &format!("{rev}:./{p}")]).ok();
    files
        .iter()
        .map(|f| {
            let new_src = match f.new.as_deref() {
                Some(_) if f.old.is_none() => added_source(f),
                Some(p) => match (a.staged, new_rev) {
                    (_, Some(r)) => show(r, p),
                    (true, None) => show("", p),
                    (false, None) => std::fs::read_to_string(p).ok(),
                },
                None => None,
            };
            let old_src = f
                .old
                .as_deref()
                .filter(|_| f.lines.iter().any(|l| !l.0))
                // Unstaged changes are relative to the index, not HEAD.
                .and_then(|p| match (a.staged, rev) {
                    (false, None) => show("", p),
                    _ => show(old_rev, p),
                });
            // Decorators and doc comments count as part of the symbol below them.
            let parse = |p: &Option<String>, src: Option<String>| {
                let p = p.as_deref()?;
                let s = src?;
                Some(outline(Lang::from_path(Path::new(p))?, &s)?.with_attached(&s, p))
            };
            // `acus outline` prints a symbol without its attached comments.
            let display_o = f.new.as_deref().filter(|_| f.old.is_none()).and_then(|p| {
                let src = new_src.as_deref()?;
                outline(Lang::from_path(Path::new(p))?, src)
            });
            let old_o = parse(&f.old, old_src.clone());
            let new_o = parse(&f.new, new_src.clone());
            View {
                f,
                old_src,
                new_src,
                old_o,
                new_o,
                display_o,
            }
        })
        .collect()
}

/// The symbol groups, text groups and new-file view of one file.
fn derive_file(view: &View<'_>, file: usize, compact: bool, index: &MoveIndex) -> Derived {
    let f = view.f;
    let symbols = groups(
        f,
        view.new_o.as_ref(),
        view.old_o.as_ref(),
        file,
        &MoveIndex::default(),
    );
    if compact && f.old.is_none() && f.new.is_some() && !f.binary {
        return Derived {
            symbols,
            text_groups: Vec::new(),
            new_file: Some(new_file(view, file, index)),
        };
    }
    let text_groups = groups(f, view.new_o.as_ref(), view.old_o.as_ref(), file, index);
    Derived {
        symbols,
        text_groups,
        new_file: None,
    }
}

/// The outline of an added file, or its first added lines when it has no symbols.
fn new_file(view: &View<'_>, file: usize, index: &MoveIndex) -> NewFile {
    let Some(outline) = view.display_o.as_ref().filter(|o| !o.symbols.is_empty()) else {
        let lines = view
            .f
            .lines
            .iter()
            .filter(|l| l.0)
            .take(FIRST_LINES)
            .cloned()
            .collect();
        return NewFile::Lines(lines);
    };
    let depth = match outline.lang {
        Lang::Toml | Lang::Json | Lang::Yaml => 1,
        _ => usize::MAX,
    };
    let rows = outline
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.depth < depth)
        .map(|(i, s)| OutlineRow {
            depth: s.depth,
            kind: s.kind,
            text: s.detail.as_deref().unwrap_or(&s.name).to_owned(),
            start: s.start_line,
            end: s.end_line,
            moved: index.at(file, true, i),
        })
        .collect();
    NewFile::Outline(rows)
}

/// Changed lines bucketed by innermost enclosing symbol, in diff order. Whole-file adds and
/// deletes form one unlabelled group; the two sides of a move stay apart.
fn groups(
    f: &FileDiff,
    new: Option<&Outline>,
    old: Option<&Outline>,
    file: usize,
    index: &MoveIndex,
) -> Vec<Group> {
    let mut v: Vec<Group> = Vec::new();
    let whole = f.old.is_none() || f.new.is_none();
    for (add, n, t) in &f.lines {
        let o = if *add { new } else { old };
        let idx = o.filter(|_| !whole).and_then(|o| enclosing_index(o, *n));
        let sym = idx.and_then(|i| o.and_then(|o| o.symbols.get(i)));
        let label = match sym {
            Some(s) => format!("{} {}", s.kind, s.qual),
            None if whole => String::new(),
            None => TOP.into(),
        };
        let moved = idx.and_then(|i| index.at(file, *add, i)).map(|m| (m, *add));
        let i = match v.iter().position(|g| g.label == label && g.moved == moved) {
            Some(i) => i,
            None => {
                // Range in the new file, so `acus show path:A-B` works; None if the symbol is gone.
                let range = if matches!(moved, Some((_, false))) {
                    None
                } else {
                    sym.and_then(|s| {
                        new?.symbols
                            .iter()
                            .find(|x| x.qual == s.qual && x.kind == s.kind)
                            .map(|x| (x.start_line, x.end_line))
                    })
                };
                v.push(Group {
                    label,
                    range,
                    add: 0,
                    del: 0,
                    lines: Vec::new(),
                    moved,
                });
                v.len() - 1
            }
        };
        let g = &mut v[i];
        if *add {
            g.add += 1;
        } else {
            g.del += 1;
        }
        g.lines.push((*add, *n, t.clone()));
    }
    v
}

/// The source of an added or untracked file, rebuilt from its added diff lines.
fn added_source(f: &FileDiff) -> Option<String> {
    if f.binary {
        return None;
    }
    let mut src = f
        .lines
        .iter()
        .filter(|(add, _, _)| *add)
        .map(|(_, _, t)| t.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if !src.is_empty() {
        src.push('\n');
    }
    Some(src)
}

fn enclosing_index(o: &Outline, line: usize) -> Option<usize> {
    o.symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.start_line <= line && line <= s.end_line)
        .max_by_key(|(_, s)| s.depth)
        .map(|(i, _)| i)
}
