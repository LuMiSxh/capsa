// SPDX-License-Identifier: MPL-2.0
//! Finds the `path:line` references in the output and the code they point at.

use super::Args;
use acus_syntax::{Lang, outline};
use acus_walk::display_path;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Returns the `path:line` references to files under `cwd`, in first-occurrence order. Handles
/// `a.rs:3:5`, `a.ts(3,5)` and Python's `File "a.py", line 3`.
pub(super) fn refs(text: &str, cwd: &Path) -> Vec<(PathBuf, usize)> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for raw in text.lines() {
        for (p, l) in candidates(raw) {
            let path = Path::new(p.trim_start_matches("./"));
            let full = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            // ponytail: only files inside the working directory; library and toolchain paths are noise.
            let Ok(rel) = full.strip_prefix(cwd) else {
                continue;
            };
            if full.is_file() && seen.insert((rel.to_path_buf(), l)) {
                v.push((rel.to_path_buf(), l));
            }
        }
    }
    v
}

fn candidates(line: &str) -> Vec<(&str, usize)> {
    let mut out = Vec::new();
    if let Some(i) = line.find("File \"") {
        let rest = &line[i + 6..];
        if let Some((p, tail)) = rest.split_once("\", line ") {
            let n: String = tail.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = n.parse() {
                out.push((p, n));
            }
        }
    }
    let b = line.as_bytes();
    let path_char = |c: u8| c.is_ascii_alphanumeric() || b"._-/\\".contains(&c);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b':' || b[i] == b'(' {
            let mut s = i;
            while s > 0 && path_char(b[s - 1]) {
                s -= 1;
            }
            // Windows drive letter: `C:\x\a.rs:3`.
            if s >= 2 && b[s - 1] == b':' && b[s - 2].is_ascii_alphabetic() && b[s] == b'\\' {
                s -= 2;
            }
            let p = &line[s..i];
            let digits: String = line[i + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            let ext = Path::new(p).extension().is_some();
            if ext && !digits.is_empty() && p.contains('.') {
                out.push((p, digits.parse().unwrap_or(0)));
            }
        }
        i += 1;
    }
    out.retain(|(_, n)| *n > 0);
    out
}

/// Returns the enclosing symbol (capped), else a few lines around `line`.
pub(super) fn snippet(path: &Path, line: usize, a: &Args) -> Option<(String, usize, Vec<String>)> {
    let src = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = src.lines().collect();
    if line > lines.len() {
        return None;
    }
    let shown = display_path(path);
    let sym = Lang::from_path(path)
        .and_then(|l| outline(l, &src))
        .and_then(|o| o.enclosing(line).cloned());
    let (label, start, end) = match sym {
        Some(s) if s.end_line - s.start_line < a.max_lines => {
            (format!("{shown}#{}", s.qual), s.start_line, s.end_line)
        }
        // Too long to print whole: the lines around the reference, still labelled.
        Some(s) => {
            let half = a.max_lines / 2;
            let st = line.saturating_sub(half).max(s.start_line);
            (
                format!("{shown}#{}", s.qual),
                st,
                (st + a.max_lines - 1).min(s.end_line),
            )
        }
        None => (
            shown,
            line.saturating_sub(a.context).max(1),
            (line + a.context).min(lines.len()),
        ),
    };
    let body = lines[start - 1..end]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    Some((label, start, body))
}

#[cfg(test)]
mod tests {
    use super::candidates;

    #[test]
    fn finds_compiler_and_runtime_references() {
        assert_eq!(candidates("  --> src/lib.rs:12:5"), [("src/lib.rs", 12)]);
        assert_eq!(
            candidates("thread 'x' panicked at crates/a/src/b.rs:7:9:"),
            [("crates/a/src/b.rs", 7)]
        );
        assert_eq!(
            candidates("src/app.ts(3,14): error TS2304"),
            [("src/app.ts", 3)]
        );
        assert_eq!(
            candidates("  File \"pkg/mod.py\", line 41, in f"),
            [("pkg/mod.py", 41)]
        );
        assert_eq!(candidates("time: 12:30 ok"), []);
    }
}
