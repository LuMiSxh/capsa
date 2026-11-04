// SPDX-License-Identifier: MPL-2.0
//! Resolves the `<<<<<<<` / `|||||||` / `=======` / `>>>>>>>` sections a patch chooses.

use super::files::Files;
use crate::error::{EditError, Result};
use crate::parse::{Op, ResolveMode};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

/// Whether `text` holds a complete git conflict section.
pub(super) fn has_markers(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    !scan(&lines).0.is_empty()
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

/// A resolved conflict section: lines `start..end` (0-based, half-open) of the file before the
/// patch, replaced by `lines`.
pub(super) struct Resolution {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) lines: Vec<String>,
}

/// Works out what each `*** Resolve Conflict` keeps, against the files as they were before the
/// patch. The caller applies these with the line directives, all from those same lines.
pub(super) fn resolutions(
    files: &mut Files,
    ops: &[&Op],
) -> Result<BTreeMap<String, Vec<Resolution>>> {
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
    let mut out = BTreeMap::new();
    for (path, state) in states {
        let resolutions: Vec<Resolution> = state
            .sections
            .iter()
            .zip(&state.chosen)
            .filter_map(|(section, chosen)| {
                chosen.as_ref().map(|lines| Resolution {
                    start: section.start,
                    end: section.end + 1,
                    lines: lines.clone(),
                })
            })
            .collect();
        if !resolutions.is_empty() {
            out.insert(path, resolutions);
        }
    }
    Ok(out)
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
}

/// The complete conflict sections of `lines`, and the first opener that never closes.
fn scan(lines: &[&str]) -> (Vec<Section>, Option<usize>) {
    let mut out = Vec::new();
    let mut unterminated = None;
    let mut i = 0;
    while i < lines.len() {
        if !is_open(lines[i]) {
            i += 1;
            continue;
        }
        match section_at(lines, i) {
            Some((section, next)) => {
                out.push(section);
                i = next;
            }
            None => {
                if unterminated.is_none() {
                    unterminated = Some(i);
                }
                i += 1;
            }
        }
    }
    (out, unterminated)
}

/// The conflict sections of `lines`, or the error naming an unterminated one.
fn sections(path: &str, lines: &[String]) -> Result<Vec<Section>> {
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (sections, unterminated) = scan(&refs);
    match unterminated {
        Some(line) => Err(EditError::UnterminatedConflict {
            path: path.into(),
            line: line + 1,
        }),
        None => Ok(sections),
    }
}

/// The section that opens at `start`, with the index after its closing marker.
fn section_at(lines: &[&str], start: usize) -> Option<(Section, usize)> {
    let mut j = start + 1;
    let ours_start = j;
    while j < lines.len() && !is_base(lines[j]) && !is_separator(lines[j]) {
        if is_close(lines[j]) {
            return None;
        }
        j += 1;
    }
    if j == lines.len() {
        return None;
    }
    let ours = (ours_start, j);
    let mut base = None;
    if is_base(lines[j]) {
        let base_start = j + 1;
        j = base_start;
        while j < lines.len() && !is_separator(lines[j]) {
            if is_close(lines[j]) {
                return None;
            }
            j += 1;
        }
        if j == lines.len() {
            return None;
        }
        base = Some((base_start, j));
    }
    // `lines[j]` is the `=======` separator.
    j += 1;
    let theirs_start = j;
    while j < lines.len() && !is_close(lines[j]) {
        j += 1;
    }
    if j == lines.len() {
        return None;
    }
    let end = j;
    Some((
        Section {
            start,
            end,
            ours,
            base,
            theirs: (theirs_start, end),
        },
        end + 1,
    ))
}

fn is_open(line: &str) -> bool {
    line.strip_prefix("<<<<<<<")
        .is_some_and(|label| label.trim() != "SEARCH")
}

fn is_close(line: &str) -> bool {
    line.strip_prefix(">>>>>>>")
        .is_some_and(|label| label.trim() != "REPLACE")
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
