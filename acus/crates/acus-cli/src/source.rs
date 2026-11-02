// SPDX-License-Identifier: MPL-2.0
//! Resolves `path`, `path:A-B` and `path#Sym` addresses to the lines they name.

use acus_syntax::{Address, Lang, Outline, Resolve, Target, attached_start, outline, resolve};
use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

/// Reads a text file the way every command names a missing or binary one.
pub fn read_text(path: &str) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => not_found(path),
        std::io::ErrorKind::InvalidData => anyhow!("{path}: binary file, not shown"),
        _ => anyhow!("{path}: {e}"),
    })
}

/// `raw` as an address. A path that exists is a file, whole, even when its name looks like an
/// address (`a:-3`, `a#b`); otherwise it is parsed as `path`, `path:A-B`, `path:-N` or
/// `path#Sym`.
pub fn address(raw: &str) -> Address {
    if Path::new(raw).exists() {
        return Address {
            path: raw.into(),
            target: Target::Whole,
        };
    }
    Address::parse(raw)
}

/// A 1-based inclusive line range and the label it is printed under.
pub struct Span {
    pub label: String,
    pub start: usize,
    pub end: usize,
}

/// Resolves an address to lines of `src`, the text of `file` (which gives the language).
/// `outlines` keeps one outline per path, so several symbols of a file outline it once.
pub fn locate(
    addr: &Address,
    file: &str,
    src: &str,
    lines: &[&str],
    outlines: &mut HashMap<String, Option<Outline>>,
) -> Result<Span> {
    let path = &addr.path;
    let (label, start, end) = match &addr.target {
        Target::Whole => (path.clone(), 1, lines.len()),
        Target::Lines(s, e) => (path.clone(), *s, *e),
        Target::From(s) => (path.clone(), *s, lines.len()),
        Target::Tail(0) => {
            return Err(capsa_core::hint::Hinted::new(
                format!("{path}:-0: tail needs at least 1 line"),
                format!("acus show {path}:-1"),
            )
            .into());
        }
        Target::Tail(n) => (
            path.clone(),
            lines.len().saturating_sub(*n) + 1,
            lines.len(),
        ),
        Target::Symbol(q) => {
            let o = outlines
                .entry(path.clone())
                .or_insert_with(|| Lang::from_path(Path::new(file)).and_then(|l| outline(l, src)));
            let Some(o) = o else {
                return Err(capsa_core::hint::Hinted::new(
                    format!("no syntax support for {path}"),
                    format!("use {path}:START-END"),
                )
                .into());
            };
            match resolve(o, q) {
                // Decorators, attributes and doc comments belong to what is shown.
                Resolve::Found(s) => (
                    format!("{path}#{}", s.qual),
                    attached_start(lines, file, s.start_line - 1) + 1,
                    s.end_line,
                ),
                Resolve::Missing => {
                    return Err(capsa_core::hint::Hinted::new(
                        format!("no symbol `{q}` in {path}"),
                        format!("acus outline {path}"),
                    )
                    .into());
                }
                Resolve::Ambiguous(v) => {
                    let c: Vec<_> = v
                        .iter()
                        .map(|s| format!("{path}:{}-{} ({})", s.start_line, s.end_line, s.qual))
                        .collect();
                    return Err(capsa_core::hint::Hinted::new(
                        format!("`{q}` is ambiguous in {path}"),
                        c.join(", "),
                    )
                    .into());
                }
            }
        }
    };
    if addr.target != Target::Whole && start == 0 {
        bail!("{path}:0: lines start at 1");
    }
    if matches!(
        addr.target,
        Target::Lines(..) | Target::From(_) | Target::Symbol(_)
    ) && (start > lines.len() || start > end)
    {
        bail!("{path} has {} lines", lines.len());
    }
    Ok(Span { label, start, end })
}

/// `REV:path` as its revision and path. A path on disk is never a revision, and the part before
/// the first colon must name a commit, so `x:-3` and `C:\…` stay plain paths.
pub fn revision(path: &str) -> Option<(&str, &str)> {
    if Path::new(path).exists() {
        return None;
    }
    let (rev, file) = path.split_once(':')?;
    if Path::new(rev).is_file() {
        return None;
    }
    let commit = Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ])
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success());
    commit.then_some((rev, file))
}

/// The text of `file` at `rev`. `git cat-file` resolves the path as git does: from the repo
/// root, or from the current directory with `REV:./path`.
pub fn show_blob(rev: &str, file: &str, path: &str) -> Result<String> {
    let o = Command::new("git")
        .args(["cat-file", "blob", &format!("{rev}:{file}")])
        .output()?;
    if !o.status.success() {
        return Err(capsa_core::hint::Hinted::new(
            format!("path not found: {path}"),
            format!(
                "{file} is not in {rev}; paths start at the repo root, and {rev}:./path is relative to the current directory"
            ),
        )
        .into());
    }
    String::from_utf8(o.stdout).map_err(|_| anyhow!("{path}: binary file, not shown"))
}

/// `path not found`, with a hint: a `REV:path` whose REV is no revision names that revision, and
/// any other path is looked up from the working directory.
pub fn not_found(path: &str) -> anyhow::Error {
    let hint = match path.split_once(':') {
        Some((rev, _)) if !Path::new(rev).is_file() => {
            format!("`{rev}` is not a revision (a commit, branch or tag)")
        }
        _ => "paths are relative to the working directory; `acus files` lists them".to_owned(),
    };
    capsa_core::hint::Hinted::new(format!("path not found: {path}"), hint).into()
}

#[cfg(test)]
mod tests {
    use super::read_text;

    #[test]
    fn read_text_names_binary_and_missing_files() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("b.dat");
        std::fs::write(&bin, [0xff, 0xfe, 0x00]).unwrap();
        let e = read_text(bin.to_str().unwrap()).unwrap_err().to_string();
        assert!(e.ends_with("binary file, not shown"), "{e}");
        assert!(read_text(d.path().join("nope.rs").to_str().unwrap()).is_err());
    }
}
