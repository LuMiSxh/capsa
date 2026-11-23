// SPDX-License-Identifier: MPL-2.0
//! Reads the files that a script's file functions see, inside the allowed roots and the Read rules.

use super::ReadOptions;
use acus_script::HostError;
use capsa_core::claude_settings::permissions::Permissions;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// Bytes searched for a NUL to decide that a file is binary, as git does.
const BINARY_PROBE: usize = 8000;

/// Extensions returned as images: a script gets their size, and the Read tool shows them.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Returns the text of the file at `path`, with the lines `options` select. A relative `path`
/// resolves against `root`, the directory the script runs in. An image or binary file returns its
/// kind, size and a hint in place of text.
pub(in super::super) fn read(
    perms: &Permissions,
    root: &Path,
    path: &str,
    options: &ReadOptions,
) -> Result<Value, HostError> {
    let file = guard(perms, root, path)?;
    let meta = std::fs::metadata(&file).map_err(|e| unreadable(path, &e))?;
    if meta.is_dir() {
        return Err(HostError::new(format!("{path} is a directory"))
            .with_hint("list directories with files()"));
    }
    if is_image(&file) {
        return Ok(media("image", path, meta.len()));
    }
    let bytes = std::fs::read(&file).map_err(|e| unreadable(path, &e))?;
    match decode(bytes) {
        Some(text) => Ok(Value::String(select(&text, options)?)),
        None => Ok(media("binary", path, meta.len())),
    }
}

/// Returns the size, kind, line count and modification time of `path`, or `exists: false` when
/// nothing is there. `lines` is absent for directories, images and binary files.
pub(in super::super) fn stat(
    perms: &Permissions,
    root: &Path,
    path: &str,
) -> Result<Value, HostError> {
    let file = guard(perms, root, path)?;
    let Ok(meta) = std::fs::metadata(&file) else {
        return Ok(json!({"exists": false, "dir": false}));
    };
    let mut info = json!({
        "exists": true,
        "dir": meta.is_dir(),
        "bytes": meta.len(),
        "modified": meta.modified().ok().map(rfc3339),
    });
    if !meta.is_dir()
        && !is_image(&file)
        && let Ok(bytes) = std::fs::read(&file)
        && let Some(text) = decode(bytes)
    {
        info["lines"] = json!(text.lines().count());
    }
    Ok(info)
}

/// Returns whether `path` exists, once it is checked against the allowed roots and the Read rules.
pub(in super::super) fn exists(
    perms: &Permissions,
    root: &Path,
    path: &str,
) -> Result<bool, HostError> {
    Ok(guard(perms, root, path)?.exists())
}

/// Returns the resolved location of `path` under `root`, once it is inside an allowed root and the
/// Read rules allow it.
fn guard(perms: &Permissions, root: &Path, path: &str) -> Result<PathBuf, HostError> {
    let raw = root.join(path);
    let joined = lexical(&raw);
    let real = resolve(&joined);
    // A `..` after a symlink leaves the repository through the link, which the lexical form
    // hides: the physical form of the path must be inside a root as well.
    let physical = resolve(&raw);
    let roots = allowed_roots(perms)?;
    let inside = |p: &Path| roots.iter().any(|dir| p.starts_with(dir));
    if !inside(&real) || !inside(&physical) {
        return Err(HostError::new(format!("{path} is outside the allowed roots")).with_hint(
            "file functions read the repository and the directories in permissions.additionalDirectories",
        ));
    }
    perms.check_read(&joined).map_err(super::super::refused)?;
    if physical != real {
        perms.check_read(&physical).map_err(super::super::refused)?;
    }
    Ok(real)
}

/// Returns the repository root and the `additionalDirectories`, resolved where they exist.
fn allowed_roots(perms: &Permissions) -> Result<Vec<PathBuf>, HostError> {
    let mut dirs = vec![perms.root().to_path_buf()];
    dirs.extend(perms.additional_dirs().map_err(super::super::refused)?);
    Ok(dirs
        .into_iter()
        .map(|dir| dir.canonicalize().unwrap_or(dir))
        .collect())
}

/// Returns `path` with `.` and `..` removed, without touching the file system.
pub(in super::super) fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Returns `path` with symlinks resolved for its longest existing ancestor, so a file that does
/// not exist yet still gets a location to compare.
pub(in super::super) fn resolve(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut current = path;
    loop {
        if let Ok(real) = current.canonicalize() {
            return tail.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// Returns the text of `bytes`, or `None` for a binary file: one with a NUL in its first bytes or
/// one that is not UTF-8.
pub(super) fn decode(bytes: Vec<u8>) -> Option<String> {
    if bytes.iter().take(BINARY_PROBE).any(|&byte| byte == 0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Returns the lines of `text` that `options` select, numbered by their place in the file when
/// `numbered` is set.
fn select(text: &str, options: &ReadOptions) -> Result<String, HostError> {
    let offset = options.offset.unwrap_or(1);
    if offset == 0 {
        return Err(HostError::new("read() offset counts lines from 1")
            .with_hint("pass offset: 1 for the first line"));
    }
    let lines = text
        .split_inclusive("\n")
        .enumerate()
        .skip(offset - 1)
        .take(options.limit.unwrap_or(usize::MAX));
    let mut out = String::new();
    for (index, line) in lines {
        if options.numbered {
            let _ = write!(out, "{}\t", index + 1);
        }
        out.push_str(line);
    }
    Ok(out)
}

fn media(kind: &str, path: &str, bytes: u64) -> Value {
    json!({"kind": kind, "path": path, "bytes": bytes, "hint": "use the Read tool to see this file"})
}

fn unreadable(path: &str, error: &std::io::Error) -> HostError {
    HostError::new(format!("cannot read {path}: {error}"))
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}
