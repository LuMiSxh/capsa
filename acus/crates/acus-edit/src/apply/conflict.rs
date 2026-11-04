// SPDX-License-Identifier: MPL-2.0
//! Resolves the `<<<<<<<` / `|||||||` / `=======` / `>>>>>>>` sections a patch chooses.

use super::Change;
use super::files::Files;
use crate::error::{EditError, Result};
use crate::parse::{Op, ResolveMode};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

/// Whether `text` holds the start or end marker of a conflict section.
pub(super) fn has_markers(text: &str) -> bool {
    text.lines()
        .any(|l| l.starts_with("<<<<<<<") || l.starts_with(">>>>>>>"))
}

/// One conflict section: its marker lines and the content ranges between them, 0-based and
/// half-open.
struct Section {
    start: usize,
    end: usize,
    ours: (usize, usize),
    base: Option<(usize, usize)>,
    theirs: (usize, usize),
}

/// The sections of one file and the replacement each directive chose; `None` keeps it as it is.
struct State {
    lines: Vec<String>,
    sections: Vec<Section>,
    chosen: Vec<Option<Vec<String>>>,
}

/// Resolves the conflict sections named by `ops`, whose line numbers refer to the files as they
/// were before the patch. The other directives of the patch run after this.
pub(super) fn apply_conflicts(files: &mut Files, ops: &[&Op]) -> Result<Vec<Change>> {
    let mut states: BTreeMap<String, State> = BTreeMap::new();
    for &op in ops {
        let Op::ResolveConflict {
            path,
            mode,
            target,
            line,
        } = op
        else {
            continue;
        };
        let state = match states.entry(path.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let lines = files.lines(path)?.clone();
                let sections = sections(path, &lines)?;
                if sections.is_empty() {
                    return Err(EditError::NoConflict { path: path.clone() });
                }
                let chosen = vec![None; sections.len()];
                entry.insert(State {
                    lines,
                    sections,
                    chosen,
                })
            }
        };
        match target {
            Some(target) => state.choose(*target, *mode, *line, path)?,
            None => state.choose_all(*mode, path)?,
        }
    }
    let mut changes = Vec::new();
    for (path, state) in states {
        if state.chosen.iter().all(Option::is_none) {
            continue;
        }
        let (lines, edits) = state.rebuild();
        let f = files.get(&path)?;
        f.lines = Some(lines);
        f.dirty = true;
        f.edits = edits;
        changes.push(Change::Modified {
            path,
            added: 0,
            removed: 0,
        });
    }
    Ok(changes)
}

impl State {
    fn choose_all(&mut self, mode: ResolveMode, path: &str) -> Result<()> {
        for i in 0..self.sections.len() {
            if self.chosen[i].is_none() {
                self.chosen[i] = Some(resolve(&self.sections[i], &self.lines, mode, path)?);
            }
        }
        Ok(())
    }

    fn choose(
        &mut self,
        target: usize,
        mode: ResolveMode,
        patch_line: usize,
        path: &str,
    ) -> Result<()> {
        let index = target
            .checked_sub(1)
            .filter(|&i| i < self.lines.len())
            .and_then(|i| {
                self.sections
                    .iter()
                    .position(|s| s.start <= i && i <= s.end)
            })
            .ok_or_else(|| EditError::NoConflictAtLine {
                path: path.into(),
                line: target,
            })?;
        if self.chosen[index].is_some() {
            return Err(EditError::ConflictAlreadyResolved {
                path: path.into(),
                line: target,
                patch_line,
            });
        }
        self.chosen[index] = Some(resolve(&self.sections[index], &self.lines, mode, path)?);
        Ok(())
    }

    /// The file with each chosen section replaced, and the (start, length) of every replacement.
    fn rebuild(&self) -> (Vec<String>, Vec<(usize, usize)>) {
        let mut out = Vec::with_capacity(self.lines.len());
        let mut edits = Vec::new();
        let mut i = 0;
        while i < self.lines.len() {
            let Some(index) = self.sections.iter().position(|s| s.start == i) else {
                out.push(self.lines[i].clone());
                i += 1;
                continue;
            };
            let section = &self.sections[index];
            match &self.chosen[index] {
                Some(replacement) => {
                    edits.push((out.len(), replacement.len()));
                    out.extend(replacement.iter().cloned());
                }
                None => out.extend(self.lines[section.start..=section.end].iter().cloned()),
            }
            i = section.end + 1;
        }
        (out, edits)
    }
}

/// The conflict sections of `lines`, or the error naming an unterminated one.
fn sections(path: &str, lines: &[String]) -> Result<Vec<Section>> {
    let unterminated = |line: usize| EditError::UnterminatedConflict {
        path: path.into(),
        line: line + 1,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].starts_with("<<<<<<<") {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = start + 1;
        let ours_start = j;
        while j < lines.len() && !is_base(&lines[j]) && !is_separator(&lines[j]) {
            if lines[j].starts_with(">>>>>>>") {
                return Err(unterminated(start));
            }
            j += 1;
        }
        if j == lines.len() {
            return Err(unterminated(start));
        }
        let ours = (ours_start, j);
        let mut base = None;
        if is_base(&lines[j]) {
            let base_start = j + 1;
            j = base_start;
            while j < lines.len() && !is_separator(&lines[j]) {
                if lines[j].starts_with(">>>>>>>") {
                    return Err(unterminated(start));
                }
                j += 1;
            }
            if j == lines.len() {
                return Err(unterminated(start));
            }
            base = Some((base_start, j));
        }
        // `lines[j]` is the `=======` separator.
        j += 1;
        let theirs_start = j;
        while j < lines.len() && !lines[j].starts_with(">>>>>>>") {
            j += 1;
        }
        if j == lines.len() {
            return Err(unterminated(start));
        }
        let end = j;
        out.push(Section {
            start,
            end,
            ours,
            base,
            theirs: (theirs_start, end),
        });
        i = end + 1;
    }
    Ok(out)
}

fn is_separator(line: &str) -> bool {
    line.trim_end() == "======="
}

fn is_base(line: &str) -> bool {
    line.starts_with("|||||||")
}

fn resolve(
    section: &Section,
    lines: &[String],
    mode: ResolveMode,
    path: &str,
) -> Result<Vec<String>> {
    let take = |(start, end): (usize, usize)| lines[start..end].to_vec();
    Ok(match mode {
        ResolveMode::Ours => take(section.ours),
        ResolveMode::Theirs => take(section.theirs),
        ResolveMode::Both => {
            let mut both = take(section.ours);
            both.extend(take(section.theirs));
            both
        }
        ResolveMode::Base => match section.base {
            Some(base) => take(base),
            None => {
                return Err(EditError::NoConflictBase {
                    path: path.into(),
                    line: section.start + 1,
                });
            }
        },
    })
}
