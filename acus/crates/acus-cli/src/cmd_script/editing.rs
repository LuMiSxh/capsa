// SPDX-License-Identifier: MPL-2.0
//! Builds the patch ops behind `write()`, `edit()` and the array form of `patch()`, and checks
//! every path a patch touches against the repository root and the `Edit(...)` rules.

use super::host::{lexical, resolve as physical};
use acus_edit::{Block, Chunk, Dest, LineAnchor, Op};
use acus_script::HostError;
use capsa_core::claude_settings::permissions::Permissions;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Resolves `path` for a change: it must lie inside the repository both by its name and with
/// symlinks followed (a `..` after a link leaves through the link), and no `Edit(...)` rule may
/// refuse either form. Returns the path with symlinks resolved, joined to `cwd`.
pub(super) fn resolve(
    permissions: &Permissions,
    cwd: &Path,
    path: &str,
) -> Result<PathBuf, HostError> {
    let raw = cwd.join(path);
    let full = lexical(&raw);
    let real = physical(&raw);
    let root = physical(permissions.root());
    if !real.starts_with(&root) || !physical(&full).starts_with(&root) {
        return Err(HostError::new(format!("{path} is outside the repository"))
            .with_hint("scripts change files under the repository root only"));
    }
    permissions.check_edit(&full).map_err(super::refused)?;
    if real != full {
        permissions.check_edit(&real).map_err(super::refused)?;
    }
    Ok(real)
}

/// Refuses an op list when a file it changes lies outside the repository or an `Edit(...)` rule
/// covers it, or a `Read(...)` rule covers a file it copies from.
pub(super) fn check(permissions: &Permissions, cwd: &Path, ops: &[Op]) -> Result<(), HostError> {
    for op in ops {
        if let Op::Copy { from, .. } = op {
            permissions
                .check_read(&cwd.join(from))
                .map_err(super::refused)?;
        }
        for path in targets(op, cwd)? {
            resolve(permissions, cwd, &path)?;
        }
    }
    Ok(())
}

/// Every path an op writes, with the files a `Replace All` directory or glob stands for.
fn targets(op: &Op, cwd: &Path) -> Result<Vec<String>, HostError> {
    Ok(match op {
        Op::Add { path, .. }
        | Op::Append { path, .. }
        | Op::Delete { path }
        | Op::ReplaceSymbol { path, .. }
        | Op::DeleteSymbol { path, .. }
        | Op::DeleteLines { path, .. }
        | Op::ResolveConflict { path, .. }
        | Op::Insert { path, .. } => vec![path.clone()],
        Op::Update { path, move_to, .. } => std::iter::once(path).chain(move_to).cloned().collect(),
        Op::ReplaceAll { paths, .. } => expand(paths, cwd)?,
        Op::MoveSymbol { path, to, .. } => {
            let mut all = vec![path.clone()];
            if let Some(Dest::Before { path, .. } | Dest::After { path, .. } | Dest::End { path }) =
                to
            {
                all.push(path.clone());
            }
            all
        }
        Op::MoveLines { path, to, .. } => {
            let mut all = vec![path.clone()];
            if let Some(LineAnchor::Line { path, .. } | LineAnchor::Symbol { path, .. }) = to {
                all.push(path.clone());
            }
            all
        }
        Op::Copy { to, .. } => to.iter().cloned().collect(),
    })
}

/// The files a `Replace All` path list reaches: plain files as named, directories and globs as the
/// patch walks them.
fn expand(specs: &[String], cwd: &Path) -> Result<Vec<String>, HostError> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut globs = Vec::new();
    for spec in specs {
        if spec.contains(['*', '?', '[']) {
            globs.push(spec.clone());
        } else if cwd.join(spec).is_dir() {
            dirs.push(cwd.join(spec));
        } else {
            files.push(spec.clone());
        }
    }
    if !dirs.is_empty() {
        files.extend(walk(dirs, Vec::new())?);
    }
    if !globs.is_empty() {
        files.extend(walk(vec![cwd.to_path_buf()], globs)?);
    }
    Ok(files)
}

fn walk(roots: Vec<PathBuf>, include: Vec<String>) -> Result<Vec<String>, HostError> {
    let found = Mutex::new(Vec::new());
    let opts = acus_walk::WalkOpts {
        roots,
        include,
        ..Default::default()
    };
    acus_walk::walk(&opts, |p| {
        found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(p.display().to_string());
    })
    .map_err(|e| HostError::new(format!("cannot list the files to edit: {e}")))?;
    Ok(found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// The text of a new file as the lines a patch holds: one trailing newline is not a line of its
/// own. `write` puts the exact bytes back after the patch.
fn lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n').map(str::to_owned).collect()
}

/// The text of a search or replacement as the lines it spans, newlines kept as they are: the
/// patch matches "a, newline" as those two characters, not as the line "a".
fn exact_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_owned).collect()
}

/// Makes the file hold exactly `text`: a patch writes whole lines, so a text without a final
/// newline gets one that the caller did not ask for.
pub(super) fn write_exact(path: &Path, text: &str) -> Result<(), HostError> {
    let fail = |e: std::io::Error| HostError::new(format!("cannot write {}: {e}", path.display()));
    if std::fs::read(path).map_err(fail)? != text.as_bytes() {
        std::fs::write(path, text).map_err(fail)?;
    }
    Ok(())
}

/// The ops that make `path` hold `text`: an add, or a delete then an add for a file that exists.
pub(super) fn write_ops(path: &str, text: &str, exists: bool) -> Vec<Op> {
    let mut ops = Vec::new();
    if exists {
        ops.push(Op::Delete {
            path: path.to_owned(),
        });
    }
    ops.push(Op::Add {
        path: path.to_owned(),
        lines: lines(text),
    });
    ops
}

/// The op that replaces `old` by `new` in `path`: once, or everywhere with `all`.
pub(super) fn edit_op(path: &str, old: &str, new: &str, all: bool) -> Result<Op, HostError> {
    if old.is_empty() {
        return Err(HostError::new("edit() needs a non-empty old text")
            .with_hint("use write(path, text) to replace a whole file"));
    }
    Ok(if all {
        Op::ReplaceAll {
            paths: vec![path.to_owned()],
            blocks: vec![Block {
                regex: false,
                search: exact_lines(old),
                replace: exact_lines(new),
            }],
        }
    } else {
        Op::Update {
            path: path.to_owned(),
            move_to: None,
            chunks: vec![Chunk {
                old: exact_lines(old),
                new: exact_lines(new),
                line: 0,
            }],
        }
    })
}

/// The ops of `patch([{path, old, new, all?}, ...])`, one per entry, applied together.
pub(super) fn batch_ops(
    items: &[Value],
    permissions: &Permissions,
    cwd: &Path,
) -> Result<Vec<Op>, HostError> {
    let shape = || {
        HostError::new("patch() takes edits as {path, old, new, all?}")
            .with_hint("patch([{path: 'a.rs', old: 'x', new: 'y'}]) edits several files at once")
    };
    let mut ops = Vec::new();
    for item in items {
        let text = |key: &str| item.get(key).and_then(Value::as_str);
        let (Some(path), Some(old), Some(new)) = (text("path"), text("old"), text("new")) else {
            return Err(shape());
        };
        resolve(permissions, cwd, path)?;
        let all = item.get("all").and_then(Value::as_bool) == Some(true);
        ops.push(edit_op(path, old, new, all)?);
    }
    Ok(ops)
}
