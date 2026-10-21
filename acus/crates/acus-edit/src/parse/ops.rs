// SPDX-License-Identifier: MPL-2.0
//! Applies directives to the ops they open or complete.

use super::addr::{insert_addr, sym_addr};
use super::directives::{Directive, Kind, accepted, did_you_mean, split_directive};
use super::{Dest, Op};
use crate::error::{EditError, Result};

/// Applies a directive that opens a new op, or sets `*** Move to` on the update before it.
fn start_op(d: &Directive, rest: &str, n: usize, ops: &mut Vec<Op>) -> Result<()> {
    match d.kind {
        Kind::Add => ops.push(Op::Add {
            path: rest.trim().into(),
            lines: vec![],
        }),
        Kind::Delete => ops.push(Op::Delete {
            path: rest.trim().into(),
        }),
        Kind::Update => ops.push(Op::Update {
            path: rest.trim().into(),
            move_to: None,
            chunks: vec![],
        }),
        Kind::MoveTo => match ops.last_mut() {
            Some(Op::Update { move_to, .. }) => *move_to = Some(rest.trim().into()),
            _ => return Err(EditError::MoveToWithoutUpdate { line: n }),
        },
        Kind::ReplaceSymbol => {
            let Some((path, symbol)) = rest.trim().split_once('#') else {
                return Err(EditError::ExpectedSymbol {
                    line: n,
                    directive: d.name.into(),
                });
            };
            ops.push(Op::ReplaceSymbol {
                path: path.into(),
                symbol: symbol.into(),
                lines: vec![],
            });
        }
        Kind::ReplaceAll => ops.push(Op::ReplaceAll {
            paths: rest.split_whitespace().map(Into::into).collect(),
            blocks: vec![],
        }),
        Kind::Copy => ops.push(Op::Copy {
            from: rest.trim().into(),
            to: None,
        }),
        Kind::Insert { after } => {
            let (path, at) = insert_addr(rest, n, d.name)?;
            ops.push(Op::Insert {
                path,
                at,
                after,
                lines: vec![],
            });
        }
        Kind::DeleteSymbol => {
            let (path, symbol) = sym_addr(rest, n, d.name)?;
            ops.push(Op::DeleteSymbol { path, symbol });
        }
        Kind::MoveSymbol => {
            let (path, symbol) = sym_addr(rest, n, d.name)?;
            ops.push(Op::MoveSymbol {
                path,
                symbol,
                to: None,
            });
        }
    }
    Ok(())
}

/// Applies `*** Before:`, `*** After:` or `*** To:`, which give the destination of the
/// `*** Move Symbol` before them, or the target of a `*** Copy File`.
fn destination(kind: &str, rest: &str, n: usize, ops: &mut [Op]) -> Result<()> {
    if let (Some(Op::Copy { to, .. }), "To") = (ops.last_mut(), kind) {
        *to = Some(rest.trim().into());
        return Ok(());
    }
    let Some(Op::MoveSymbol { to, .. }) = ops.last_mut() else {
        return Err(if kind == "To" {
            EditError::ToWithoutMoveOrCopy { line: n }
        } else {
            EditError::DestinationWithoutMove {
                line: n,
                directive: kind.into(),
            }
        });
    };
    *to = Some(match kind {
        "To" => Dest::End {
            path: rest.trim().into(),
        },
        _ => {
            let (path, symbol) = sym_addr(rest, n, kind)?;
            if kind == "Before" {
                Dest::Before { path, symbol }
            } else {
                Dest::After { path, symbol }
            }
        }
    });
    Ok(())
}

/// Handles a line that is a directive or a destination line (`*** Before:`, `*** After:`,
/// `*** To:`). Returns false for any other line, which is block body text. An unknown `***`
/// line is an error.
pub(super) fn directive(line: &str, n: usize, ops: &mut Vec<Op>) -> Result<bool> {
    if let Some((d, rest)) = split_directive(line) {
        start_op(d, rest, n, ops)?;
    } else if let Some((kind, rest)) = ["Before", "After", "To"]
        .iter()
        .find_map(|k| line.strip_prefix(&format!("*** {k}: ")).map(|r| (*k, r)))
    {
        destination(kind, rest, n, ops)?;
    } else if line.starts_with("***") {
        return Err(EditError::UnknownDirective {
            line: n,
            text: line.into(),
            hint: format!("{}accepted: {}", did_you_mean(line), accepted()),
        });
    } else {
        return Ok(false);
    }
    Ok(true)
}
