// SPDX-License-Identifier: MPL-2.0
//! Renders an added or untracked file as an outline in `acus diff -p`.

use super::moves::{Move, MoveIndex, moved_line};
use super::render::{FILE_LINES, Style, View};
use acus_syntax::Lang;
use capsa_core::render::{Tone, paint};

/// Lines printed from a new file that has no outline.
const FIRST_LINES: usize = 20;

/// Appends the outline, or the first lines when there is none, and names the full body.
/// Returns the changed lines dropped by the per-file cap.
pub(super) fn render(
    block: &mut Vec<String>,
    style: &Style<'_>,
    view: &View<'_>,
    file: usize,
    moves: &[Move],
    index: &MoveIndex,
) -> usize {
    let path = view.f.new.as_deref().unwrap_or_default();
    let mut dropped = 0;
    match view.display_o.as_ref().filter(|o| !o.symbols.is_empty()) {
        Some(outline) => {
            let depth = match outline.lang {
                Lang::Toml | Lang::Json | Lang::Yaml => 1,
                _ => usize::MAX,
            };
            let mut printed = 0;
            for (i, s) in outline.symbols.iter().enumerate() {
                if s.depth >= depth {
                    continue;
                }
                let text = s.detail.as_deref().unwrap_or(&s.name);
                if let Some(m) = index.at(file, true, i) {
                    block.push(moved_line(&moves[m], &format!("{} {text}", s.kind)));
                    for (add, n, t) in &moves[m].lines {
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
                } else {
                    block.push(format!(
                        "  {}{} {text} {}-{}",
                        "  ".repeat(s.depth),
                        s.kind,
                        s.start_line,
                        s.end_line
                    ));
                }
            }
        }
        None => {
            for (_, n, t) in view
                .f
                .lines
                .iter()
                .filter(|(add, _, _)| *add)
                .take(FIRST_LINES)
            {
                block.push(style.line(true, *n, t));
            }
        }
    }
    block.push(
        paint(
            &format!("… body: acus show {path}"),
            Tone::Dim,
            style.out.color,
        )
        .into_owned(),
    );
    dropped
}
