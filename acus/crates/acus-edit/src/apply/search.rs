// SPDX-License-Identifier: MPL-2.0
//! SEARCH-block matching: exact first, then with line-number prefixes, trailing whitespace or
//! leading indentation ignored.

use crate::Chunk;
use crate::error::{EditError, Result};

/// A fallback that a SEARCH block needed to match.
pub(super) enum Fix {
    /// The block's `acus show` line-number prefixes are removed.
    Numbers,
    /// Trailing whitespace is ignored.
    Trailing,
    /// Leading indentation is ignored, and REPLACE is written in the file's indentation.
    Indent,
}

/// One replacement of a SEARCH block: the `old` lines from `at` become `new`.
pub(super) struct Edit {
    pub(super) at: usize,
    pub(super) old: usize,
    pub(super) new: Vec<String>,
    pub(super) added: usize,
    pub(super) removed: usize,
}

/// Byte offsets where `needle` occurs in `text`, overlapping occurrences included.
fn occurrences(text: &str, needle: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let Some(i) = text[at..].find(needle) else {
            break;
        };
        let hit = at + i;
        found.push(hit);
        // Step one char, not one byte, past the hit: overlapping matches still count.
        at = hit + text[hit..].chars().next().map_or(1, char::len_utf8);
    }
    found
}

/// Every occurrence of `search` in `lines`, each as the edit that replaces it by `replace`.
/// With `trim`, lines are compared without trailing whitespace, so `search` must be given that
/// way too.
fn search_edits(lines: &[String], search: &str, replace: &str, trim: bool) -> Vec<Edit> {
    let view: Vec<&str> = lines
        .iter()
        .map(|l| if trim { l.trim_end() } else { l.as_str() })
        .collect();
    let text = view.join("\n");
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line_of = |b: usize| starts.partition_point(|&s| s <= b) - 1;
    occurrences(&text, search)
        .into_iter()
        .map(|ms| {
            let me = ms + search.len();
            let (first, last) = (line_of(ms), line_of(me));
            let head = &lines[first][..ms - starts[first]];
            let tail = &lines[last][me - starts[last]..];
            let new: Vec<String> = format!("{head}{replace}{tail}")
                .split('\n')
                .map(Into::into)
                .collect();
            let old = &lines[first..=last];
            let (added, removed) = change_stats(old, &new);
            Edit {
                at: first,
                old: old.len(),
                new,
                added,
                removed,
            }
        })
        .collect()
}

/// Added and removed line counts of replacing `old` by `new`, not counting the lines they
/// share at either end.
fn change_stats(old: &[String], new: &[String]) -> (usize, usize) {
    let pre = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suf = old[pre..]
        .iter()
        .rev()
        .zip(new[pre..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (new.len() - pre - suf, old.len() - pre - suf)
}

/// `line` without the `acus show` line-number prefix (`12<TAB>`, with leading spaces or a `>`
/// marker allowed), if it has one.
fn strip_number(line: &str) -> Option<&str> {
    let t = line.trim_start_matches(' ');
    let t = t.strip_prefix('>').unwrap_or(t);
    let digits = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let rest = &t[digits..];
    rest.strip_prefix('>').unwrap_or(rest).strip_prefix('\t')
}

/// `lines` without their line-number prefixes, if every non-empty line has one.
fn unnumbered(lines: &[String]) -> Option<Vec<String>> {
    lines
        .iter()
        .map(|l| match strip_number(l) {
            Some(rest) => Some(rest.to_owned()),
            None if l.is_empty() => Some(String::new()),
            None => None,
        })
        .collect()
}

/// Columns of the leading indentation of `line`, a tab counting as four.
fn indent_cols(line: &str) -> usize {
    line.chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// The leading indentation of `line`, as written.
fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Each non-blank line's indent as its rank among the distinct indents of `lines`. Equal ranks
/// mean the same nesting level, whatever the indent unit; blank lines have no rank.
fn levels(lines: &[String]) -> Vec<Option<usize>> {
    let mut widths: Vec<usize> = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_cols(l))
        .collect();
    widths.sort_unstable();
    widths.dedup();
    lines
        .iter()
        .map(|l| {
            (!l.trim().is_empty()).then(|| {
                widths
                    .binary_search(&indent_cols(l))
                    .unwrap_or_else(|rank| rank)
            })
        })
        .collect()
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// `new` (the REPLACE lines) written in the indentation of `window`, the lines that the SEARCH
/// block `old` matched ignoring indentation. Each line's indent relative to the block's first
/// line is scaled from the block's indent unit to the window's, and written with the window's
/// indent characters (tabs or spaces).
fn reindent(window: &[String], old: &[String], new: &[String]) -> Vec<String> {
    let first = |lines: &[String]| {
        lines
            .iter()
            .find(|l| !l.trim().is_empty())
            .map_or(0, |l| indent_cols(l))
    };
    let (base_s, base_f) = (first(old), first(window));
    let unit = |lines: &[String], base: usize| {
        lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .fold(0, |g, l| gcd(g, indent_cols(l).abs_diff(base)))
    };
    // A block with a single indent level has no unit: borrow the other side's.
    let (us, uf) = match (unit(old, base_s), unit(window, base_f)) {
        (0, 0) => (1, 1),
        (0, f) => (f, f),
        (s, 0) => (s, s),
        units => units,
    };
    let tabs = window.iter().any(|l| indent_of(l).contains('\t'));
    new.iter()
        .map(|l| {
            if l.trim().is_empty() {
                return String::new();
            }
            let rel = indent_cols(l) as i64 - base_s as i64;
            let cols = (base_f as i64 + rel * uf as i64 / us as i64).max(0) as usize;
            let indent = if tabs {
                format!("{}{}", "\t".repeat(cols / 4), " ".repeat(cols % 4))
            } else {
                " ".repeat(cols)
            };
            format!("{indent}{}", l.trim_start())
        })
        .collect()
}

/// Every window of `lines` that equals the SEARCH block once leading indentation is ignored and
/// nests the same way: a line deeper than another stays deeper, and equal indents stay equal.
/// Each comes as the edit that writes REPLACE in the window's indentation.
fn indented_edits(lines: &[String], c: &Chunk) -> Vec<Edit> {
    let n = c.old.len();
    if n == 0 || n > lines.len() {
        return Vec::new();
    }
    let want = levels(&c.old);
    (0..=lines.len() - n)
        .filter(|&i| {
            let window = &lines[i..i + n];
            window.iter().zip(&c.old).all(|(f, s)| f.trim() == s.trim()) && levels(window) == want
        })
        .map(|i| {
            let window = &lines[i..i + n];
            let new = reindent(window, &c.old, &c.new);
            let (added, removed) = change_stats(window, &new);
            Edit {
                at: i,
                old: n,
                new,
                added,
                removed,
            }
        })
        .collect()
}

/// Applies the SEARCH block `c` (block `n` of its file) to `lines`: its text must occur exactly
/// once in the file. Each fallback runs only when the attempt before it found nothing, and each
/// must also give exactly one match.
pub(super) fn search_block(
    lines: &[String],
    c: &Chunk,
    path: &str,
    n: usize,
) -> Result<(Edit, Option<Fix>)> {
    let (search, replace) = (c.old.join("\n"), c.new.join("\n"));
    let mut found = search_edits(lines, &search, &replace, false);
    let mut fix = None;
    if found.is_empty()
        && let Some(old) = unnumbered(&c.old)
    {
        let new = unnumbered(&c.new).unwrap_or_else(|| c.new.clone());
        found = search_edits(lines, &old.join("\n"), &new.join("\n"), false);
        fix = Some(Fix::Numbers);
    }
    if found.is_empty() {
        let trimmed = c
            .old
            .iter()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        found = search_edits(lines, &trimmed, &replace, true);
        fix = Some(Fix::Trailing);
    }
    if found.is_empty() {
        // Only a unique window counts: two equal ones are ambiguous, so they stay a failure.
        let indented = indented_edits(lines, c);
        if indented.len() == 1 {
            found = indented;
            fix = Some(Fix::Indent);
        }
    }
    match found.len() {
        0 => {}
        1 => return Ok((found.swap_remove(0), fix)),
        _ => {
            return Err(EditError::SearchAmbiguous {
                path: path.into(),
                block: n,
                lines: found.iter().map(|e| e.at + 1).collect(),
            });
        }
    }
    let first = c
        .old
        .iter()
        .find(|l| !l.trim().is_empty())
        .map_or("", |l| l.trim());
    Err(EditError::SearchNotFound {
        path: path.into(),
        block: n,
        first: first.into(),
        nearest: nearest(lines, &c.old, path),
    })
}

/// The window of `lines` most like `old`, as `nearest: PATH:A-B` plus its first lines, so a
/// failed SEARCH block can be fixed in one step.
fn nearest(lines: &[String], old: &[String], path: &str) -> String {
    const SHOWN: usize = 8;
    let n = old.len().min(lines.len());
    if n == 0 {
        return String::new();
    }
    let want: Vec<_> = old.iter().map(|l| bigrams(l)).collect();
    let have: Vec<_> = lines.iter().map(|l| bigrams(l)).collect();
    let mut best: Option<(f32, usize)> = None;
    for i in 0..=lines.len() - n {
        let score: f32 = (0..n)
            .map(|k| similarity(&old[k], &lines[i + k], &want[k], &have[i + k]))
            .sum();
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, i));
        }
    }
    let Some((_, i)) = best.filter(|&(s, _)| s / n as f32 >= 0.4) else {
        return String::new();
    };
    let span = if n == 1 {
        format!("{}", i + 1)
    } else {
        format!("{}-{}", i + 1, i + n)
    };
    let mut out = format!("\nnearest: {path}:{span}");
    for k in 0..n.min(SHOWN) {
        out.push_str(&format!("\n{}\t{}", i + k + 1, lines[i + k]));
    }
    if n > SHOWN {
        out.push_str(&format!("\n… {} more lines", n - SHOWN));
    }
    out
}

/// Sorted character pairs of the trimmed line.
fn bigrams(line: &str) -> Vec<(char, char)> {
    let cs: Vec<char> = line.trim().chars().collect();
    let mut v: Vec<_> = cs.windows(2).map(|w| (w[0], w[1])).collect();
    v.sort_unstable();
    v
}

/// Similarity from 0 to 1: 1 for equal text ignoring surrounding whitespace, else the Dice
/// coefficient of the bigrams.
fn similarity(a: &str, b: &str, ba: &[(char, char)], bb: &[(char, char)]) -> f32 {
    if a.trim() == b.trim() {
        return 1.0;
    }
    if ba.is_empty() || bb.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < ba.len() && j < bb.len() {
        match ba[i].cmp(&bb[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    2.0 * common as f32 / (ba.len() + bb.len()) as f32
}
