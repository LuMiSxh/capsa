// SPDX-License-Identifier: MPL-2.0
//! The patterns of `acus find`: the `-e` values or the first positional, with grep's `\|`.

use super::Args;
use anyhow::Result;

/// The patterns: each `-e`, else the first positional; reads `\|` as alternation unless `-E`,
/// `-P` or `-F` is given.
pub(super) fn take_patterns(a: &Args) -> Result<Vec<String>> {
    let mut patterns: Vec<String> = if a.patterns.is_empty() {
        a.positional.first().cloned().into_iter().collect()
    } else {
        a.patterns.clone()
    };
    if patterns.is_empty() {
        return Err(capsa_core::hint::Hinted::new(
            "no pattern given",
            "acus find PATTERN [PATH...] or -e PAT -e PAT",
        )
        .into());
    }
    if !a.fixed_strings && !a.perl && !a._extended {
        let mut noted = false;
        for p in &mut patterns {
            if let Some(fixed) = bre_alternation(p) {
                *p = fixed;
                noted = true;
            }
        }
        if noted {
            eprintln!(
                "note: read `\\|` as alternation `|` (grep BRE habit); write `[|]` for a literal pipe, or pass -E/-P/-F to keep `\\|` as is"
            );
        }
    }
    Ok(patterns)
}

/// grep's BRE spells alternation `\|`; Rust regex reads it as a literal pipe. Agents write the
/// grep form from habit, so unless the pattern already uses an unescaped `|` (a sign the author
/// knows ERE) each `\|` outside a character class becomes `|`. `\\|` is an escaped backslash
/// followed by such a `|`, so it leaves the pattern as is. Returns None when nothing changes.
fn bre_alternation(p: &str) -> Option<String> {
    let mut out = String::with_capacity(p.len());
    let (mut in_class, mut changed) = (false, false);
    let mut chars = p.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('|') if !in_class => {
                    out.push('|');
                    changed = true;
                }
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            },
            '[' if !in_class => {
                in_class = true;
                out.push(c);
            }
            ']' if in_class => {
                in_class = false;
                out.push(c);
            }
            '|' if !in_class => return None,
            _ => out.push(c),
        }
    }
    changed.then_some(out)
}
