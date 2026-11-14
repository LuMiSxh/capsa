// SPDX-License-Identifier: MPL-2.0
//! Applies a parsed patch to in-memory copies of its files, then writes them all or none.

mod commit;
mod conflict;
mod files;
mod line_ranges;
mod ops;
mod replace_all;
mod search;
mod symbols;

use crate::Op;
use crate::error::{EditError, Result};
use commit::{check_syntax, commit};
use files::Files;
use line_ranges::apply_line_edits;
use ops::apply_op;
use similar::{ChangeTag, TextDiff};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

/// One line of the summary printed after a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added {
        path: String,
        lines: usize,
    },
    Deleted {
        path: String,
        lines: usize,
    },
    Modified {
        path: String,
        added: usize,
        removed: usize,
    },
    Moved {
        from: String,
        to: String,
        added: usize,
        removed: usize,
    },
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Change::Added { path, lines } => write!(f, "A {path} +{lines}"),
            Change::Deleted { path, lines } => write!(f, "D {path} -{lines}"),
            Change::Modified {
                path,
                added,
                removed,
            } => write!(f, "M {path} +{added} -{removed}"),
            Change::Moved {
                from,
                to,
                added,
                removed,
            } => write!(f, "R {from} -> {to} +{added} -{removed}"),
        }
    }
}

/// Lines a patch wrote, as they read afterwards (`start` is 1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub path: String,
    pub start: usize,
    pub lines: Vec<String>,
}

/// The text of a file the patch writes or deletes, before and after, for `--diff`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileText {
    pub path: String,
    /// `None` for a file the patch creates.
    pub before: Option<String>,
    /// `None` for a file the patch deletes. LF line ends, as the patch sees them.
    pub after: Option<String>,
}

/// What `apply` did: one summary per op plus the edited regions of updated files.
#[derive(Debug, Default)]
pub struct Applied {
    pub changes: Vec<Change>,
    pub regions: Vec<Region>,
    /// `path block N` of every SEARCH block that matched only after its line-number prefixes were
    /// removed.
    pub numbered: Vec<String>,
    /// `path block N` of every SEARCH block that matched only ignoring trailing whitespace.
    pub trailing: Vec<String>,
    /// `path block N` of every SEARCH block that matched only ignoring leading indentation; its
    /// REPLACE was written in the file's indentation.
    pub indented: Vec<String>,
    /// `path block N at A-B` of every SEARCH block that matched only ignoring whitespace
    /// differences (`--fuzzy`); its REPLACE was written as given.
    pub fuzzy: Vec<String>,
    /// Patch line of every SEARCH block whose SEARCH equals its REPLACE: checked, left as it is.
    pub skipped: Vec<usize>,
    /// `path block N` of every multi-line SEARCH matched in a CRLF file, whose text is compared
    /// with LF line ends.
    pub crlf: Vec<String>,
    /// Paths written although their syntax got worse (`--allow-broken`).
    pub broken: Vec<String>,
    /// Paths whose syntax was not checked because they still hold conflict markers.
    pub unchecked: Vec<String>,
    /// Before and after text of every file the patch wrote or deleted.
    pub texts: Vec<FileText>,
}

impl Applied {
    /// Recounts `changes` from `texts`; call it after changing `texts`, as `--fmt` does.
    pub fn recount(&mut self) {
        self.changes = settle(std::mem::take(&mut self.changes), &self.texts);
    }
}

/// Options for one apply pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApplyOptions {
    /// Validate and report; write nothing.
    pub check: bool,
    /// Write a file that gains syntax errors.
    pub allow_broken: bool,
    /// Accept SEARCH blocks that differ from the file only in whitespace.
    pub fuzzy: bool,
}

/// Validates and applies every op in memory, then writes all files atomically.
/// With `options.check`, stops before writing. Relative paths resolve against `root`.
/// With `options.allow_broken`, a file whose syntax gets worse is written instead of failing.
pub fn apply(ops: &[Op], root: &Path, options: &ApplyOptions) -> Result<Applied> {
    let mut files = Files {
        root,
        map: BTreeMap::new(),
        numbered: Vec::new(),
        trailing: Vec::new(),
        indented: Vec::new(),
        fuzzy_matches: Vec::new(),
        fuzzy: options.fuzzy,
        skipped: Vec::new(),
        crlf: Vec::new(),
    };
    // Conflict and line directives number the file as it was before the patch, so they are
    // worked out together against those lines and applied first; the rest then apply in patch
    // order to the result.
    let (conflict_ops, rest): (Vec<&Op>, Vec<&Op>) = ops
        .iter()
        .partition(|op| matches!(op, Op::ResolveConflict { .. }));
    let (line_ops, other_ops): (Vec<&Op>, Vec<&Op>) = rest
        .iter()
        .partition(|op| matches!(op, Op::DeleteLines { .. } | Op::MoveLines { .. }));
    let mut changes = apply_line_edits(&mut files, &conflict_ops, &line_ops)?;
    // Every op runs so one failure does not hide the next. A failed block is skipped, so the
    // blocks after it see the file without it; nothing is written while any failure remains.
    let mut failures = Vec::new();
    for op in other_ops {
        match apply_op(&mut files, op) {
            Ok(mut made) => changes.append(&mut made),
            Err(EditError::Many { failures: mut more }) => failures.append(&mut more),
            Err(e) => failures.push(e),
        }
    }
    if failures.len() == 1 {
        return Err(failures.remove(0));
    }
    if !failures.is_empty() {
        return Err(EditError::many(failures));
    }
    let mut regions = Vec::new();
    for (path, f) in &files.map {
        let (Some(lines), true) = (&f.lines, f.existed) else {
            continue;
        };
        let mut edits = f.edits.clone();
        edits.sort_unstable();
        for (s, n) in edits.into_iter().filter(|&(_, n)| n > 0) {
            regions.push(Region {
                path: path.clone(),
                start: s + 1,
                lines: lines[s..s + n].to_vec(),
            });
        }
    }
    let texts: Vec<FileText> = files
        .map
        .iter()
        .filter(|(_, f)| f.dirty)
        .map(|(path, f)| FileText {
            path: path.clone(),
            before: if f.existed { f.before.clone() } else { None },
            after: f.lines.as_ref().map(|lines| {
                let mut text = lines.join("\n");
                if f.trailing_newline && !lines.is_empty() {
                    text.push('\n');
                }
                text
            }),
        })
        .collect();
    let (broken, unchecked) = check_syntax(&files, options.allow_broken)?;
    if !options.check {
        commit(root, std::mem::take(&mut files.map))?;
    }
    Ok(Applied {
        changes: settle(changes, &texts),
        regions,
        numbered: files.numbered,
        trailing: files.trailing,
        indented: files.indented,
        fuzzy: files.fuzzy_matches,
        skipped: files.skipped,
        crlf: files.crlf,
        broken,
        unchecked,
        texts,
    })
}

/// One summary line per file: the patch's net change to it, counted from `texts` as `git diff
/// --numstat` counts it. A file the patch changes in several steps, or edits and moves, gets
/// one line, from the path it had at the start to the path it ends at.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the caller hands over its change list"
)]
fn settle(changes: Vec<Change>, texts: &[FileText]) -> Vec<Change> {
    let text = |p: &str| texts.iter().find(|t| t.path == p);
    // Each file the patch touches: its path at the start, its path at the end, and whether the
    // start path held it then (a path a move vacates and a later step reuses holds a new file).
    let mut files: Vec<Option<(String, String, bool)>> = Vec::new();
    // The index into `files` of the file now at each path.
    let mut at: BTreeMap<String, usize> = BTreeMap::new();
    let mut started = BTreeSet::new();
    for c in &changes {
        let (from, to) = match c {
            Change::Moved { from, to, .. } => (from.as_str(), to.as_str()),
            Change::Added { path, .. }
            | Change::Deleted { path, .. }
            | Change::Modified { path, .. } => (path.as_str(), path.as_str()),
        };
        let idx = at.remove(from).unwrap_or_else(|| {
            let first = started.insert(from.to_owned());
            files.push(Some((from.to_owned(), from.to_owned(), first)));
            files.len() - 1
        });
        if let Some((_, end, _)) = files[idx].as_mut() {
            *end = to.to_owned();
        }
        // A file moved onto a deleted one takes over its entry.
        if let Some(old) = at.insert(to.to_owned(), idx)
            && old != idx
        {
            files[old] = None;
        }
    }
    let mut out = Vec::new();
    for (start, end, first) in files.into_iter().flatten() {
        let before = if first {
            text(&start).and_then(|t| t.before.as_deref())
        } else {
            None
        };
        let after = text(&end).and_then(|t| t.after.as_deref());
        let (added, removed) = line_counts(before.unwrap_or(""), after.unwrap_or(""));
        out.push(match (before.is_some(), after.is_some(), start == end) {
            (false, false, _) => continue,
            (false, true, _) => Change::Added {
                path: end,
                lines: added,
            },
            (true, false, _) => Change::Deleted {
                path: start,
                lines: removed,
            },
            (true, true, true) => Change::Modified {
                path: end,
                added,
                removed,
            },
            (true, true, false) => Change::Moved {
                from: start,
                to: end,
                added,
                removed,
            },
        });
    }
    out
}

/// Added and removed lines from `before` to `after`; CRLF counts as LF.
fn line_counts(before: &str, after: &str) -> (usize, usize) {
    let (before, after) = (before.replace("\r\n", "\n"), after.replace("\r\n", "\n"));
    TextDiff::from_lines(before.as_str(), after.as_str())
        .iter_all_changes()
        .fold((0, 0), |(added, removed), c| match c.tag() {
            ChangeTag::Insert => (added + 1, removed),
            ChangeTag::Delete => (added, removed + 1),
            ChangeTag::Equal => (added, removed),
        })
}
