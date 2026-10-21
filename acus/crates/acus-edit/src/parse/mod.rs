// SPDX-License-Identifier: MPL-2.0
//! Parses patch text into ops, one per directive.

mod addr;
mod body;
mod directives;
mod ops;

use crate::error::{EditError, Result};
use body::{Stage, body};
use ops::directive;

#[derive(Debug, Clone, PartialEq, Eq)]
/// One directive of a patch, in the order the directives appear. The lines after a directive
/// (its `+` lines, SEARCH blocks or `*** To:`) fill in the op before them, so the parser builds
/// each op in place.
pub enum Op {
    /// Creates a file from the `+` lines that follow.
    Add {
        path: String,
        lines: Vec<String>,
    },
    /// Removes a file. It takes no body, so the op is complete as soon as its line is read.
    Delete {
        path: String,
    },
    /// Edits a file in place. A following `*** Move to` sets `move_to`, which keeps a rename in the
    /// same op; an update with no chunks is rejected by `parse`.
    Update {
        path: String,
        move_to: Option<String>,
        chunks: Vec<Chunk>,
    },
    /// Replaces a whole symbol. The symbol is named rather than given a line number, so the
    /// target still resolves after edits above it.
    ReplaceSymbol {
        path: String,
        symbol: String,
        lines: Vec<String>,
    },
    /// Every occurrence of each block's search text in every listed file.
    ReplaceAll {
        paths: Vec<String>,
        blocks: Vec<Block>,
    },
    DeleteSymbol {
        path: String,
        symbol: String,
    },
    MoveSymbol {
        path: String,
        symbol: String,
        to: Option<Dest>,
    },
    /// Inserts the `+` lines next to a symbol or line.
    Insert {
        path: String,
        at: InsertAt,
        after: bool,
        lines: Vec<String>,
    },
    /// Copies a file: `*** Copy File: from` then `*** To: to`.
    Copy {
        from: String,
        to: Option<String>,
    },
}

/// The anchor of `*** Insert Before/After`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertAt {
    Symbol(String),
    /// The 1-based line number.
    Line(usize),
}

/// A `<<<<<<< SEARCH` (or `REGEX`) / `=======` / `>>>>>>> REPLACE` block of `*** Replace All`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Block {
    pub regex: bool,
    pub search: Vec<String>,
    pub replace: Vec<String>,
}

/// Where `*** Move Symbol` puts the symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dest {
    Before { path: String, symbol: String },
    After { path: String, symbol: String },
    End { path: String },
}

/// One `<<<<<<< SEARCH` block of `*** Update File`: `old` must occur exactly once in the file
/// and is replaced by `new`. A block whose `old` equals `new` is checked, then skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chunk {
    pub old: Vec<String>,
    pub new: Vec<String>,
    /// The patch line of its `<<<<<<< SEARCH` line.
    pub line: usize,
}

/// Expands a leading `~` in every path the ops name: no shell runs on patch text.
fn expand_home(ops: &mut [Op]) {
    fn home(p: &mut String) {
        *p = capsa_core::paths::expand_home(p);
    }
    for op in ops {
        match op {
            Op::Add { path, .. }
            | Op::Delete { path }
            | Op::ReplaceSymbol { path, .. }
            | Op::DeleteSymbol { path, .. }
            | Op::Insert { path, .. } => home(path),
            Op::Update { path, move_to, .. } => {
                home(path);
                if let Some(p) = move_to {
                    home(p);
                }
            }
            Op::ReplaceAll { paths, .. } => paths.iter_mut().for_each(home),
            Op::MoveSymbol { path, to, .. } => {
                home(path);
                if let Some(dest) = to {
                    match dest {
                        Dest::Before { path, .. }
                        | Dest::After { path, .. }
                        | Dest::End { path } => home(path),
                    }
                }
            }
            Op::Copy { from, to } => {
                home(from);
                if let Some(p) = to {
                    home(p);
                }
            }
        }
    }
}

/// Parses patch text into ops, in the order their directives appear. Errors name the patch line
/// and carry a `hint:` line, so the caller can fix the patch and resend it.
pub fn parse(text: &str) -> Result<Vec<Op>> {
    let mut ops: Vec<Op> = Vec::new();
    let mut stage: Stage = None;
    for (i, line) in text.trim_end().lines().enumerate() {
        let n = i + 1;
        // Block contents are verbatim, even lines that look like directives.
        if stage.is_some() {
            body(ops.last_mut(), line, n, &mut stage)?;
            continue;
        }
        if line == "*** Begin Patch" || line == "*** End Patch" {
            continue;
        }
        if !directive(line, n, &mut ops)? {
            body(ops.last_mut(), line, n, &mut stage)?;
        }
    }
    if stage.is_some() {
        return Err(EditError::UnterminatedBlock);
    }
    for op in &ops {
        if let Op::Update {
            path,
            move_to: None,
            chunks,
        } = op
            && chunks.is_empty()
        {
            return Err(EditError::NoChanges { path: path.clone() });
        }
        match op {
            Op::ReplaceAll { paths, blocks } if paths.is_empty() || blocks.is_empty() => {
                return Err(EditError::ReplaceAllEmpty);
            }
            Op::MoveSymbol {
                path,
                symbol,
                to: None,
            } => {
                return Err(EditError::MoveSymbolNoDestination {
                    path: path.clone(),
                    symbol: symbol.clone(),
                });
            }
            Op::Copy { from, to: None } => {
                return Err(EditError::CopyFileNoDestination { from: from.clone() });
            }
            Op::Insert { path, lines, .. } if lines.is_empty() => {
                return Err(EditError::InsertNoLines { path: path.clone() });
            }
            _ => {}
        }
    }
    if ops.is_empty() {
        return Err(EditError::EmptyPatch);
    }
    let nothing_to_write = ops.iter().all(|op| {
        matches!(op, Op::Update { move_to: None, chunks, .. } if !chunks.is_empty() && chunks.iter().all(|c| c.old == c.new))
    });
    if nothing_to_write {
        let line = match &ops[0] {
            Op::Update { chunks, .. } => chunks[0].line,
            _ => 0,
        };
        return Err(EditError::IdenticalBlock { line });
    }
    expand_home(&mut ops);
    Ok(ops)
}
