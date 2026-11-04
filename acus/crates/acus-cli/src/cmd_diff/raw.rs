// SPDX-License-Identifier: MPL-2.0
//! Parses `git diff -U0` output into per-file changed lines.

use super::FileDiff;

/// Parses `git diff -U0` with `a/` and `b/` prefixes.
pub(super) fn parse(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut o, mut n, mut in_hunk) = (0, 0, false);
    for l in text.lines() {
        if let Some(h) = l.strip_prefix("diff --git a/") {
            let (a, b) = h.split_once(" b/").unwrap_or((h, h));
            files.push(FileDiff {
                old: Some(a.into()),
                new: Some(b.into()),
                ..FileDiff::default()
            });
            in_hunk = false;
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if in_hunk {
            if let Some(t) = l.strip_prefix('-') {
                f.lines.push((false, o, t.into()));
                o += 1;
            } else if let Some(t) = l.strip_prefix('+') {
                f.lines.push((true, n, t.into()));
                n += 1;
            }
        }
        if let Some(h) = l.strip_prefix("@@ -") {
            in_hunk = true;
            let num = |s: &str| {
                s.split(',')
                    .next()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0)
            };
            let mut it = h.split(' ');
            o = num(it.next().unwrap_or(""));
            n = num(it.next().unwrap_or("").trim_start_matches('+'));
        } else if in_hunk {
        } else if l.starts_with("new file mode") {
            f.old = None;
        } else if l.starts_with("deleted file mode") {
            f.new = None;
        } else if let Some(p) = l.strip_prefix("rename from ") {
            f.old = Some(p.into());
        } else if let Some(p) = l.strip_prefix("rename to ") {
            f.new = Some(p.into());
        } else if let Some(p) = l.strip_prefix("--- a/") {
            f.old = Some(p.trim_end_matches('\t').into());
        } else if let Some(p) = l.strip_prefix("+++ b/") {
            f.new = Some(p.trim_end_matches('\t').into());
        } else if l.starts_with("Binary files ") {
            f.binary = true;
        }
    }
    files
}
