// SPDX-License-Identifier: MPL-2.0
//! Owns the shell writes into source files that `acus patch` or the Write tool replace.

use super::*;

/// Returns the refusal of a `cat`, `printf`, `echo` or `tee` segment that writes a source file
/// inside the repository. Only `>`/`>>` redirects and `tee` arguments count; a heredoc body is
/// not part of `seg`.
pub(super) fn file_write(seg: &str, scope: Option<&Scope>) -> Option<Verdict> {
    if !seg.contains('>') && !seg.contains("tee") {
        return None;
    }
    let (head, words) = head(seg)?;
    let words: Vec<&str> = words.collect();
    let target = match head {
        // `cat FILE > OUT` copies or joins files: nothing is authored there.
        "cat" if has_operand(&words) => None,
        "cat" | "printf" | "echo" => redirect_targets(&words).find(|t| is_repo_source(t, scope)),
        "tee" => words
            .iter()
            .copied()
            .filter(|w| !w.starts_with('-'))
            .find(|t| is_repo_source(t, scope)),
        _ => None,
    }?;
    Some(Verdict::Deny(
        Rule::FileWrite,
        format!(
            "Use acus instead of writing {} from the shell: `acus patch` with `*** Add File: PATH` creates a new file and `*** Update File: PATH` edits one (SEARCH/REPLACE blocks); the Write tool also creates a new file. {ESCAPE}",
            unquote(target)
        ),
    ))
}

/// Reports whether a word is a file operand: not a flag and not part of a redirect.
fn has_operand(words: &[&str]) -> bool {
    let mut it = words.iter();
    while let Some(w) = it.next() {
        let op = w.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
        if op.starts_with("<<") {
            // `<<`, `<<-` and `<<<` take the next word as delimiter or string when spaced.
            if op.trim_start_matches(['<', '-']).is_empty() {
                it.next();
            }
        } else if op.starts_with('<') {
            // `< FILE` reads a file: a copy, not authored content.
            return true;
        } else if op.starts_with('>') {
            if op.trim_start_matches(['>', '|']).is_empty() {
                it.next();
            }
        } else if !w.starts_with('-') {
            return true;
        }
    }
    false
}

/// Yields the files that `>`, `>>` and `>|` send stdout to; `2>`, `>&2` and `&>` are not stdout
/// writes of the command itself.
fn redirect_targets<'a>(words: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    words.iter().enumerate().filter_map(|(i, w)| {
        let w = w
            .strip_prefix('1')
            .filter(|r| r.starts_with('>'))
            .unwrap_or(w);
        let rest = w.strip_prefix('>')?.trim_start_matches(['>', '|']);
        if rest.starts_with('&') {
            return None;
        }
        if rest.is_empty() {
            words.get(i + 1).copied()
        } else {
            Some(rest)
        }
    })
}

/// Reports whether `path` names a source file inside the repository. A path that starts with a
/// variable (`$TMPDIR/x.rs`) is unknown and passes. Markdown is prose that notes and changelogs
/// write whole, so it does not count.
pub(super) fn is_repo_source(path: &str, scope: Option<&Scope>) -> bool {
    let p = unquote(path);
    let prose = p
        .rsplit_once('.')
        .is_some_and(|(_, e)| ["md", "markdown", "mdx"].contains(&e.to_ascii_lowercase().as_str()));
    !p.starts_with('$') && !prose && is_source_path(p) && inside(scope, p)
}
