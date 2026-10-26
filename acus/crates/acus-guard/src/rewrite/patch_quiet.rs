// SPDX-License-Identifier: MPL-2.0

//! Folds an `acus patch` whose written lines are discarded into `acus patch -q`.

use super::*;

/// Folds the `acus patch` segment at `i` when a `head` or `tail` that ends the pipeline, or a
/// redirect to `/dev/null`, throws its output away. It runs before `fold_at`, whose opaque test
/// would keep a segment with a stdout redirect.
pub(super) fn fold(segs: &[Segment], i: usize) -> Option<Fold> {
    let (last, new) = replacement(segs, i)?;
    Some(Fold {
        last,
        rule: Rule::PatchQuiet,
        new,
        from_command: false,
    })
}

/// Returns (last folded index, replacement text) for the `acus patch` segment at `i`.
fn replacement(segs: &[Segment], i: usize) -> Option<(usize, String)> {
    let seg = &segs[i];
    if seg.nest != Nest::Top || seg.assigns > 0 {
        return None;
    }
    let text = seg.orig.trim();
    match seg.out {
        Out::Pipe => {
            let next = segs.get(i + 1)?;
            let words = split_words(next.low_unescaped().1);
            let ["head" | "tail", ..] = words.as_slice() else {
                return None;
            };
            if next.opaque()
                || next.heredoc
                || next.assigns > 0
                || !seg.feeds(next)
                || next.out != Out::Agent
            {
                return None;
            }
            Some((i + 1, quiet(text)?))
        }
        Out::File => Some((i, quiet(without_dev_null(text)?)?)),
        _ => None,
    }
}

/// Returns `text` with `-q` after `acus patch`, unless `-q` or `--quiet` is already there.
fn quiet(text: &str) -> Option<String> {
    let words = split_words(text);
    let ["acus", "patch", ..] = words.as_slice() else {
        return None;
    };
    if words.iter().any(|w| matches!(*w, "-q" | "--quiet")) {
        return Some(text.to_owned());
    }
    let end = words[1].as_ptr() as usize - text.as_ptr() as usize + words[1].len();
    Some(format!("{} -q{}", &text[..end], &text[end..]))
}

/// Returns `text` without its stdout redirect to `/dev/null`, and without a `2>&1` after it, so
/// stderr stays visible. Any other redirect gives `None`.
fn without_dev_null(text: &str) -> Option<&str> {
    let text = text.strip_suffix("2>&1").map_or(text, str::trim_end);
    let text = text.strip_suffix("/dev/null")?.trim_end();
    let text = text
        .strip_suffix("&>")
        .or_else(|| text.strip_suffix('>'))?
        .trim_end();
    // A digit before the `>` makes it `2>`, a redirect of stderr.
    (!text.is_empty() && !text.contains('>') && !text.ends_with(|c: char| c.is_ascii_digit()))
        .then_some(text)
}
