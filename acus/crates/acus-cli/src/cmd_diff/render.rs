// SPDX-License-Identifier: MPL-2.0
//! Builds the per-file views of `acus diff` and prints the symbol groups, the text view and JSON.

use super::moves::{Move, MoveIndex, find_moves, moved_line};
use super::status::Status;
use super::{Args, FileDiff, counts, file_path, modes, new_files, status, total};
use crate::git::git;
use crate::out::{Budget, Out, clip};
use acus_syntax::{Lang, Outline, outline};
use capsa_core::format::Format;
use capsa_core::render::{Tone, paint};
use std::path::Path;

/// Printed changed lines per file with `-p`; `--full` lifts the cap.
pub(super) const FILE_LINES: usize = 80;
/// Label for changed lines outside any symbol.
const TOP: &str = "(top level)";

struct Group {
    label: String,
    range: Option<(usize, usize)>,
    add: usize,
    del: usize,
    lines: Vec<(bool, usize, String)>,
    /// The move this group belongs to and whether it is the destination side.
    moved: Option<(usize, bool)>,
}

/// A changed file with the sources and outlines the text view needs beyond its diff lines.
pub(super) struct View<'a> {
    pub(super) f: &'a FileDiff,
    pub(super) old_src: Option<String>,
    pub(super) new_src: Option<String>,
    pub(super) old_o: Option<Outline>,
    pub(super) new_o: Option<Outline>,
    /// The new-side outline as `acus outline` prints it, without attached comments.
    pub(super) display_o: Option<Outline>,
}

/// Each changed file with the sources and outlines the text view needs.
pub(super) fn views<'a>(
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

/// The JSON value: the status header when there is one, then one object per changed file.
pub(super) fn print_json(
    a: &Args,
    header: Option<&Status>,
    files: &[FileDiff],
    views: &[View<'_>],
) {
    let none = MoveIndex::default();
    let v: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let syms: Vec<serde_json::Value> = views.get(i).map_or_else(Vec::new, |view| {
                groups(f, view.new_o.as_ref(), view.old_o.as_ref(), i, &none)
                    .iter()
                    .map(|g| {
                        let mut s = serde_json::json!({
                            "symbol": g.label, "range": g.range, "added": g.add, "removed": g.del,
                        });
                        if a.lines {
                            s["lines"] = g.lines.iter().map(|(add, n, t)| serde_json::json!({
                                "op": if *add { "+" } else { "-" }, "line": n, "text": t,
                            })).collect();
                        }
                        s
                    })
                    .collect()
            });
            serde_json::json!({
                "status": status(f), "path": f.new.as_ref().or(f.old.as_ref()), "old_path": f.old,
                "binary": f.binary, "added": total(f, true), "removed": total(f, false), "symbols": syms,
            })
        })
        .collect();
    let mut root = serde_json::Map::new();
    if let Some(h) = header {
        root.insert("header".into(), h.json());
    }
    root.insert("files".into(), serde_json::Value::Array(v));
    outln!("{}", serde_json::Value::Object(root));
}

/// Output style for the text view.
pub(super) struct Style<'a> {
    pub(super) out: &'a Out,
    lines: bool,
    pub(super) compact: bool,
    cut: Option<usize>,
}

impl Style<'_> {
    pub(super) fn line(&self, add: bool, n: usize, t: &str) -> String {
        let t = self.cut.map_or_else(|| t.to_owned(), |c| clip(t, 0, c));
        if add {
            self.out.numbered(n, &format!("+{t}"), false)
        } else {
            format!("\t-{t}")
        }
    }
}

/// The text view: one block per file, cut at `--max-lines` and `--head`, then a total and the
/// footer.
pub(super) fn print_text_files(
    a: &Args,
    out: &Out,
    header: Option<&Status>,
    files: &[FileDiff],
    views: &[View<'_>],
    staged: Option<String>,
    rev: Option<&str>,
) {
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    if let Some(h) = header {
        budget.emit(&out.header(&h.header()));
    }
    let style = Style {
        out,
        lines: a.lines,
        compact: a.lines && !a.full,
        cut: a.cut.map(|n| n as usize),
    };
    let moves = if style.compact {
        find_moves(views)
    } else {
        Vec::new()
    };
    let index = MoveIndex::new(&moves);
    let mut printed = 0;
    // The human view lines up the paths and the counts of the file headers.
    let path_w = views
        .iter()
        .map(|v| file_path(v.f).chars().count())
        .max()
        .unwrap_or(0);
    let count_w = views
        .iter()
        .map(|v| counts(total(v.f, true), total(v.f, false)).chars().count())
        .max()
        .unwrap_or(0);
    for (i, view) in views.iter().enumerate() {
        let f = view.f;
        let path = file_path(f);
        let count = counts(total(f, true), total(f, false));
        let tail = if count.is_empty() {
            if f.binary {
                " (binary)".to_owned()
            } else {
                String::new()
            }
        } else if out.format == Format::Human {
            let binary = if f.binary { " (binary)" } else { "" };
            format!("  {count:>count_w$}{binary}")
        } else {
            let binary = if f.binary { "(binary)" } else { "" };
            format!(" {count}{binary}")
        };
        let head = if out.format == Format::Human {
            format!("{} {path:<path_w$}{tail}", status(f))
        } else {
            format!("{} {path}{tail}", status(f))
        };
        let mut block = vec![out.header(&head).trim_end().to_owned()];
        let dropped = if style.compact && f.old.is_none() && f.new.is_some() && !f.binary {
            new_files::render(&mut block, &style, view, i, &moves, &index)
        } else {
            text_groups(&mut block, &style, view, i, &moves, &index)
        };
        if dropped > 0 {
            block.push(
                paint(
                    &format!("… {dropped} more changed lines: {}", rerun(a, rev, f)),
                    Tone::Dim,
                    out.color,
                )
                .into_owned(),
            );
        }
        if printed > 0 && printed + block.len() > a.max_lines {
            budget.emit(&format!(
                "… {} more files (limit with -- PATH, or drop -p)",
                views.len() - i
            ));
            break;
        }
        printed += block.len();
        block.iter().for_each(|l| budget.emit(l));
    }
    let (add, del) = files.iter().fold((0, 0), |(x, y), f| {
        (x + total(f, true), y + total(f, false))
    });
    budget.emit(&out.header(&modes::total_line(files.len(), add, del)));
    if let Some(line) = staged {
        budget.emit(&paint(&line, Tone::Dim, out.color));
    }
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
}

/// Appends the symbol groups of a modified or deleted file, honouring the per-file cap.
/// Returns the changed lines dropped by the cap.
fn text_groups(
    block: &mut Vec<String>,
    style: &Style<'_>,
    view: &View<'_>,
    file: usize,
    moves: &[Move],
    index: &MoveIndex,
) -> usize {
    let mut printed = 0;
    let mut dropped = 0;
    for g in groups(
        view.f,
        view.new_o.as_ref(),
        view.old_o.as_ref(),
        file,
        index,
    ) {
        let header = match g.moved {
            Some((m, true)) => Some(moved_line(&moves[m], &g.label)),
            Some((m, false)) => summary(&g).map(|s| format!("{s} moved → {}", moves[m].to_path)),
            None => summary(&g),
        };
        if let Some(line) = header {
            block.push(line);
        }
        if !style.lines {
            continue;
        }
        let lines: &[(bool, usize, String)] = match g.moved {
            Some((m, true)) => &moves[m].lines,
            Some(_) => &[],
            None => &g.lines,
        };
        for (add, n, t) in lines {
            if style.compact && t.trim().is_empty() {
                continue;
            }
            if style.compact && printed >= FILE_LINES {
                dropped += 1;
                continue;
            }
            block.push(style.line(*add, *n, t));
            printed += 1;
        }
    }
    dropped
}

/// The symbol summary line, or None for whole-file groups.
fn summary(g: &Group) -> Option<String> {
    if g.label.is_empty() {
        return None;
    }
    let range = match g.range {
        Some((s, e)) => format!(" {s}-{e}"),
        None if g.label == TOP => String::new(),
        None => " (removed)".into(),
    };
    Some(format!("  {}{range} {}", g.label, counts(g.add, g.del)))
}

/// The rerun that shows every changed line of a file; `--full` lifts the per-file cap.
fn rerun(a: &Args, rev: Option<&str>, f: &FileDiff) -> String {
    let mut cmd = String::from("acus diff -p --full");
    if let Some(r) = rev {
        cmd.push(' ');
        cmd.push_str(r);
    }
    if a.staged {
        cmd.push_str(" --staged");
    }
    cmd.push_str(" -- ");
    cmd.push_str(f.new.as_deref().or(f.old.as_deref()).unwrap_or_default());
    cmd
}

/// Changed lines bucketed by innermost enclosing symbol, in diff order. Whole-file adds and
/// deletes form one unlabelled group; the two sides of a move stay apart.
fn groups(
    f: &FileDiff,
    new: Option<&Outline>,
    old: Option<&Outline>,
    file: usize,
    moves: &MoveIndex,
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
        let moved = idx.and_then(|i| moves.at(file, *add, i)).map(|m| (m, *add));
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
