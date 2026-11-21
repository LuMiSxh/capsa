// SPDX-License-Identifier: MPL-2.0
//! Prints an added file in the text view of `acus diff -p`: its outline, or its first lines.

use super::moves::{Move, moved_line};
use super::report::NewFile;
use super::text::{FILE_LINES, Style};
use capsa_core::render::{Tone, paint};

/// Appends the outline or first lines of an added file, then names its full body. Returns the
/// changed lines dropped by the per-file cap.
pub(super) fn render(
    block: &mut Vec<String>,
    style: &Style<'_>,
    nf: &NewFile,
    moves: &[Move],
    path: &str,
) -> usize {
    let mut dropped = 0;
    match nf {
        NewFile::Outline(rows) => {
            let mut printed = 0;
            for row in rows {
                let Some(m) = row.moved else {
                    block.push(format!(
                        "  {}{} {} {}-{}",
                        "  ".repeat(row.depth),
                        row.kind,
                        row.text,
                        row.start,
                        row.end
                    ));
                    continue;
                };
                block.push(moved_line(&moves[m], &format!("{} {}", row.kind, row.text)));
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
            }
        }
        NewFile::Lines(lines) => {
            for (_, n, t) in lines {
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
