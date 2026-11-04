// SPDX-License-Identifier: MPL-2.0
//! Pairs symbols removed in one place and added elsewhere in the same diff.

use super::{View, file_path};
use acus_syntax::Outline;
use similar::{ChangeTag, TextDiff};
use std::collections::{HashMap, HashSet, VecDeque};

/// One symbol that left one place and arrived in another.
pub(super) struct Move {
    pub(super) from_file: usize,
    pub(super) from_sym: usize,
    pub(super) to_file: usize,
    pub(super) to_sym: usize,
    pub(super) from_path: String,
    pub(super) to_path: String,
    /// The removed symbol's line range in its old file.
    pub(super) from_range: (usize, usize),
    /// The body's changed lines; empty when only indentation or trailing whitespace differs.
    pub(super) lines: Vec<(bool, usize, String)>,
}

/// Maps each end of a move to its index, so changed lines group by side.
#[derive(Default)]
pub(super) struct MoveIndex {
    from: HashMap<(usize, usize), usize>,
    to: HashMap<(usize, usize), usize>,
}

impl MoveIndex {
    pub(super) fn new(moves: &[Move]) -> Self {
        let mut index = Self::default();
        for (i, m) in moves.iter().enumerate() {
            index.from.insert((m.from_file, m.from_sym), i);
            index.to.insert((m.to_file, m.to_sym), i);
        }
        index
    }

    pub(super) fn at(&self, file: usize, added: bool, sym: usize) -> Option<usize> {
        let side = if added { &self.to } else { &self.from };
        side.get(&(file, sym)).copied()
    }
}

struct Candidate {
    file: usize,
    sym: usize,
    body: String,
    kind: &'static str,
    name: String,
}

/// Finds whole symbol ranges removed and added again, in the same diff and at any place.
/// Bodies equal after trimming indentation and trailing whitespace count as unchanged.
pub(super) fn find_moves(views: &[View<'_>]) -> Vec<Move> {
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for (file, view) in views.iter().enumerate() {
        collect(view, file, false, &mut removed);
        collect(view, file, true, &mut added);
    }
    pair(&removed, &added)
        .into_iter()
        .filter_map(|(o, n)| build(views, &removed[o], &added[n]))
        .collect()
}

/// The moved symbol's label, with the destination on the right of the arrow.
pub(super) fn moved_line(mv: &Move, destination: &str) -> String {
    let state = if mv.lines.is_empty() {
        "unchanged"
    } else {
        "edited"
    };
    let (start, end) = mv.from_range;
    format!(
        "  moved {destination} ← {}:{start}-{end} ({state})",
        mv.from_path
    )
}

fn collect(view: &View<'_>, file: usize, added: bool, out: &mut Vec<Candidate>) {
    let (outline, src) = if added {
        (view.new_o.as_ref(), view.new_src.as_deref())
    } else {
        (view.old_o.as_ref(), view.old_src.as_deref())
    };
    let (Some(outline), Some(src)) = (outline, src) else {
        return;
    };
    let touched: HashSet<usize> = view
        .f
        .lines
        .iter()
        .filter(|(add, _, _)| *add == added)
        .map(|(_, n, _)| *n)
        .collect();
    for (i, s) in outline.symbols.iter().enumerate() {
        if !is_leaf(outline, i) || !covers(&touched, s.start_line, s.end_line) {
            continue;
        }
        let Some(body) = body(src, s.start_line, s.end_line) else {
            continue;
        };
        out.push(Candidate {
            file,
            sym: i,
            body: normalize(&body),
            kind: s.kind,
            name: s.name.clone(),
        });
    }
}

fn body(src: &str, start: usize, end: usize) -> Option<Vec<&str>> {
    let lines: Vec<&str> = src.lines().collect();
    let from = start.checked_sub(1)?;
    if from >= end || end > lines.len() {
        return None;
    }
    Some(lines[from..end].to_vec())
}

fn normalize(body: &[&str]) -> String {
    body.iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n")
}

fn covers(touched: &HashSet<usize>, start: usize, end: usize) -> bool {
    start <= end && (start..=end).all(|line| touched.contains(&line))
}

fn is_leaf(outline: &Outline, i: usize) -> bool {
    let s = &outline.symbols[i];
    !outline.symbols.iter().enumerate().any(|(j, child)| {
        j != i
            && child.depth > s.depth
            && child.start_line >= s.start_line
            && child.end_line <= s.end_line
    })
}

fn pair(removed: &[Candidate], added: &[Candidate]) -> Vec<(usize, usize)> {
    let mut used = vec![false; removed.len()];
    let mut matched = vec![false; added.len()];
    let mut pairs = Vec::new();
    let mut by_body: HashMap<&str, VecDeque<usize>> = HashMap::new();
    for (i, c) in removed.iter().enumerate() {
        by_body.entry(c.body.as_str()).or_default().push_back(i);
    }
    for (i, c) in added.iter().enumerate() {
        if let Some(old) = by_body
            .get_mut(c.body.as_str())
            .and_then(VecDeque::pop_front)
        {
            used[old] = true;
            matched[i] = true;
            pairs.push((old, i));
        }
    }
    let mut by_name: HashMap<(&str, &str), VecDeque<usize>> = HashMap::new();
    for (i, c) in removed.iter().enumerate() {
        if !used[i] {
            by_name
                .entry((c.kind, c.name.as_str()))
                .or_default()
                .push_back(i);
        }
    }
    for (i, c) in added.iter().enumerate() {
        if matched[i] {
            continue;
        }
        if let Some(old) = by_name
            .get_mut(&(c.kind, c.name.as_str()))
            .and_then(VecDeque::pop_front)
        {
            used[old] = true;
            pairs.push((old, i));
        }
    }
    pairs
}

fn build(views: &[View<'_>], old: &Candidate, new: &Candidate) -> Option<Move> {
    let old_view = views.get(old.file)?;
    let new_view = views.get(new.file)?;
    let old_sym = old_view.old_o.as_ref()?.symbols.get(old.sym)?;
    let new_sym = new_view.new_o.as_ref()?.symbols.get(new.sym)?;
    // A symbol rewritten where it stood is not a move.
    if old.file == new.file
        && old_sym.start_line <= new_sym.end_line
        && new_sym.start_line <= old_sym.end_line
    {
        return None;
    }
    let old_body = body(
        old_view.old_src.as_deref()?,
        old_sym.start_line,
        old_sym.end_line,
    )?;
    let new_body = body(
        new_view.new_src.as_deref()?,
        new_sym.start_line,
        new_sym.end_line,
    )?;
    Some(Move {
        from_file: old.file,
        from_sym: old.sym,
        to_file: new.file,
        to_sym: new.sym,
        from_path: file_path(old_view.f),
        to_path: file_path(new_view.f),
        from_range: (old_sym.start_line, old_sym.end_line),
        lines: diff(&old_body, old_sym.start_line, &new_body, new_sym.start_line),
    })
}

fn diff(
    old: &[&str],
    old_start: usize,
    new: &[&str],
    new_start: usize,
) -> Vec<(bool, usize, String)> {
    let old_key: Vec<&str> = old.iter().map(|line| line.trim()).collect();
    let new_key: Vec<&str> = new.iter().map(|line| line.trim()).collect();
    let diff = TextDiff::from_slices(&old_key, &new_key);
    let mut lines = Vec::new();
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => {
                if let Some(i) = change.old_index() {
                    lines.push((false, old_start + i, old[i].to_owned()));
                }
            }
            ChangeTag::Insert => {
                if let Some(i) = change.new_index() {
                    lines.push((true, new_start + i, new[i].to_owned()));
                }
            }
            ChangeTag::Equal => {}
        }
    }
    lines
}
