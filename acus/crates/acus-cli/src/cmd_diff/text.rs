// SPDX-License-Identifier: MPL-2.0
//! Prints the text view of `acus diff`: one block per changed file, then the total and footer.

use super::added;
use super::lists;
use super::moves::{Move, moved_line};
use super::report::{Changes, FileReport, Group, TOP};
use super::{Args, FileDiff, counts, file_path, staged_line, status, total};
use crate::out::{Budget, Out, clip};
use capsa_core::format::Format;
use capsa_core::render::{Tone, paint};

/// Printed changed lines per file with `-p`; `--full` lifts the cap.
pub(super) const FILE_LINES: usize = 80;

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

/// Prints the header, one block per file cut at `--max-lines` and `--head`, then a total and the
/// footer.
pub(super) fn print(a: &Args, out: &Out, c: &Changes) {
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    if let Some(h) = &c.header {
        budget.emit(&out.header(&h.header()));
    }
    let style = Style {
        out,
        lines: a.lines,
        compact: a.lines && !a.full,
        cut: a.cut.map(|n| n as usize),
    };
    let mut printed = 0;
    // The human view lines up the paths and the counts of the file headers.
    let path_w = c
        .files
        .iter()
        .map(|f| file_path(&f.diff).chars().count())
        .max()
        .unwrap_or(0);
    let count_w = c
        .files
        .iter()
        .map(|f| {
            counts(total(&f.diff, true), total(&f.diff, false))
                .chars()
                .count()
        })
        .max()
        .unwrap_or(0);
    for (i, f) in c.files.iter().enumerate() {
        let path = file_path(&f.diff);
        let count = counts(total(&f.diff, true), total(&f.diff, false));
        let tail = if count.is_empty() {
            if f.diff.binary {
                " (binary)".to_owned()
            } else {
                String::new()
            }
        } else if out.format == Format::Human {
            let binary = if f.diff.binary { " (binary)" } else { "" };
            format!("  {count:>count_w$}{binary}")
        } else {
            let binary = if f.diff.binary { "(binary)" } else { "" };
            format!(" {count}{binary}")
        };
        let head = if out.format == Format::Human {
            format!("{} {path:<path_w$}{tail}", status(&f.diff))
        } else {
            format!("{} {path}{tail}", status(&f.diff))
        };
        let mut block = vec![out.header(&head).trim_end().to_owned()];
        let dropped = match &f.new_file {
            Some(nf) => added::render(
                &mut block,
                &style,
                nf,
                &c.moves,
                f.diff.new.as_deref().unwrap_or_default(),
            ),
            None => text_groups(&mut block, &style, f, &c.moves),
        };
        if dropped > 0 {
            block.push(
                paint(
                    &format!(
                        "… {dropped} more changed lines: {}",
                        rerun(a, c.rev.as_deref(), &f.diff)
                    ),
                    Tone::Dim,
                    out.color,
                )
                .into_owned(),
            );
        }
        if printed > 0 && printed + block.len() > a.max_lines {
            budget.emit(&format!(
                "… {} more files (limit with -- PATH, or drop -p)",
                c.files.len() - i
            ));
            break;
        }
        printed += block.len();
        block.iter().for_each(|l| budget.emit(l));
    }
    let (add, del) = c.files.iter().fold((0, 0), |(x, y), f| {
        (x + total(&f.diff, true), y + total(&f.diff, false))
    });
    budget.emit(&out.header(&lists::total_line(c.files.len(), add, del)));
    if let Some(s) = &c.staged {
        budget.emit(&paint(&staged_line(s), Tone::Dim, out.color));
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
    f: &FileReport,
    moves: &[Move],
) -> usize {
    let mut printed = 0;
    let mut dropped = 0;
    for g in &f.text_groups {
        let header = match g.moved {
            Some((m, true)) => Some(moved_line(&moves[m], &g.label)),
            Some((m, false)) => summary(g).map(|s| format!("{s} moved → {}", moves[m].to_path)),
            None => summary(g),
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
