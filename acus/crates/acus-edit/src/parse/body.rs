// SPDX-License-Identifier: MPL-2.0
//! Reads the `+` lines and SEARCH/REPLACE blocks that follow a directive.

use super::{Block, Chunk, Op};
use crate::error::{EditError, Result};

/// Inside a SEARCH/REPLACE block: false while reading the search half, true for the
/// replacement, plus how many blocks the replacement itself opened (so it can contain example
/// blocks).
pub(super) type Stage = Option<(bool, u32)>;

#[expect(
    clippy::needless_pass_by_value,
    reason = "the op is a per-line reborrow of the chunk"
)]
pub(super) fn body(op: Option<&mut Op>, line: &str, n: usize, stage: &mut Stage) -> Result<()> {
    let opens = |l: &str| ["<<<<<<< SEARCH", "<<<<<<<"].contains(&l.trim_end());
    let closes = |l: &str| [">>>>>>> REPLACE", ">>>>>>>"].contains(&l.trim_end());
    let ends = matches!(stage, Some((true, 0))) && closes(line);
    if let Some((true, d)) = stage {
        if opens(line) || line.trim_end() == "<<<<<<< REGEX" {
            *d += 1;
        } else if closes(line) && *d > 0 {
            *d -= 1;
        }
    }
    match op {
        Some(Op::ReplaceAll { blocks, .. }) => match *stage {
            None if opens(line) || line.trim_end() == "<<<<<<< REGEX" => {
                blocks.push(Block {
                    regex: line.trim_end().ends_with("REGEX"),
                    ..Block::default()
                });
                *stage = Some((false, 0));
            }
            None => return Err(EditError::ExpectedBlock { line: n }),
            Some((false, _)) if line.trim_end() == "=======" => *stage = Some((true, 0)),
            Some((true, _)) if ends => *stage = None,
            Some((r, _)) => {
                let Some(b) = blocks.last_mut() else {
                    return Err(EditError::TextOutsideBlock { line: n });
                };
                if r { &mut b.replace } else { &mut b.search }.push(line.into());
            }
        },
        Some(Op::Update { chunks, .. }) if stage.is_some() || opens(line) => match *stage {
            None => {
                chunks.push(Chunk {
                    line: n,
                    ..Chunk::default()
                });
                *stage = Some((false, 0));
            }
            Some((false, _)) if line.trim_end() == "=======" => *stage = Some((true, 0)),
            Some((true, _)) if ends => {
                let Some(c) = chunks.last() else {
                    return Err(EditError::TextOutsideChunk { line: n });
                };
                if c.old.join("\n").is_empty() {
                    return Err(EditError::EmptySearch { line: n });
                }
                *stage = None;
            }
            Some((r, _)) => {
                let Some(c) = chunks.last_mut() else {
                    return Err(EditError::TextOutsideChunk { line: n });
                };
                if r { &mut c.new } else { &mut c.old }.push(line.into());
            }
        },
        Some(
            Op::Add { lines, .. } | Op::ReplaceSymbol { lines, .. } | Op::Insert { lines, .. },
        ) => match line.strip_prefix('+') {
            Some(l) => lines.push(l.into()),
            None => return Err(EditError::NotAddLine { line: n }),
        },
        Some(Op::Update { .. }) => {
            if line.trim().is_empty() {
                return Ok(());
            }
            if line.starts_with("<<<<<<< REGEX") {
                return Err(EditError::RegexUnderUpdate { line: n });
            }
            if line.starts_with(['@', '-', '+', ' ']) {
                return Err(EditError::HunkLines { line: n });
            }
            return Err(EditError::ExpectedSearch {
                line: n,
                got: line.into(),
            });
        }
        Some(Op::Delete { path }) => {
            return Err(EditError::UnexpectedAfter {
                line: n,
                after: format!("`*** Delete File: {path}`"),
            });
        }
        Some(Op::DeleteSymbol { path, symbol } | Op::MoveSymbol { path, symbol, .. }) => {
            return Err(EditError::UnexpectedAfter {
                line: n,
                after: format!("`{path}#{symbol}`"),
            });
        }
        Some(Op::Copy { from, .. }) => {
            return Err(EditError::UnexpectedAfter {
                line: n,
                after: format!("`*** Copy File: {from}`"),
            });
        }
        None => return Err(EditError::ContentBeforeDirective { line: n }),
    }
    Ok(())
}
