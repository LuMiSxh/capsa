// SPDX-License-Identifier: MPL-2.0
//! Folds an `acus …` call and its display filters into the call's own flags.

use super::*;
use crate::read::{Window, window};

/// Folds an `acus …` segment at `i` and its following display filters. Returns (last folded
/// index, replacement text for `segs[i..=last]`).
pub(crate) fn fold(segs: &[Segment], i: usize) -> Option<(usize, String)> {
    let seg = &segs[i];
    if seg.opaque() || seg.stdin || seg.heredoc || seg.assigns > 0 || seg.out != Out::Pipe {
        return None;
    }
    let text = seg.orig.trim();
    let text = text.strip_suffix(" 2>&1").unwrap_or(text).trim_end();
    let words = split_words(text);
    if !matches!(
        words.as_slice(),
        ["acus", "show" | "outline" | "find" | "diff", ..]
    ) || words.iter().any(|w| refused_word(unquote(w)))
    {
        return None;
    }
    // A cut before the head moves behind it; a cut after the head stays behind.
    let mut cut = None;
    let mut at = i + 1;
    let mut next = stage(segs, i, at)?;
    if cut_stage(next) {
        cut = Some(next.orig.trim());
        at += 1;
        next = stage(segs, at - 1, at)?;
    }
    let hw = split_words(next.low_unescaped().1);
    let ["head", opts @ ..] = hw.as_slice() else {
        return None;
    };
    let Some(Window::First(n)) = window("head", opts) else {
        return None;
    };
    let mut last = at;
    if next.out == Out::Pipe {
        let tail = stage(segs, at, at + 1).filter(|t| cut.is_none() && cut_stage(t))?;
        cut = Some(tail.orig.trim());
        last = at + 1;
        if segs[last].out != Out::Agent {
            return None;
        }
    } else if next.out != Out::Agent {
        return None;
    }
    let mut new = format!("{text} --head {n}");
    if let Some(cut) = cut {
        new.push_str(" | ");
        new.push_str(cut);
    }
    Some((last, new))
}

/// Returns the segment at `to` when `segs[from]` pipes into it as a plain top-level stage.
fn stage(segs: &[Segment], from: usize, to: usize) -> Option<&Segment> {
    segs.get(to)
        .filter(|n| !n.opaque() && !n.heredoc && n.assigns == 0 && segs[from].feeds(n))
}

fn cut_stage(seg: &Segment) -> bool {
    matches!(split_words(seg.low_unescaped().1).as_slice(), ["cut", c] if c.starts_with("-c") && c.len() > 2)
}

/// Lists the flags that change the output format or already limit it, and redirects.
fn refused_word(w: &str) -> bool {
    // clap prints help and exits before it looks at `--head`.
    matches!(
        w,
        "--json" | "--format" | "--head" | "--cut" | "--help" | "-h" | "help"
    ) || w.starts_with("--format=")
        || w.starts_with("--head=")
        || w.starts_with("--cut=")
        || w.trim_start_matches(|c: char| c.is_ascii_digit())
            .starts_with(['>', '<', '&'])
}

#[cfg(all(test, any(feature = "bash", feature = "zsh")))]
mod tests {
    fn rw(cmd: &str) -> Option<String> {
        crate::rewrite::rewrite(cmd, None).map(|r| r.command)
    }

    #[test]
    fn folds_head_into_the_flag() {
        for (cmd, want) in [
            ("acus show a.rs | head -50", "acus show a.rs --head 50"),
            (
                "acus find 'fn x' src 2>&1 | head -n 20",
                "acus find 'fn x' src --head 20",
            ),
            (
                "acus outline src | head -30 | cut -c1-120",
                "acus outline src --head 30 | cut -c1-120",
            ),
            (
                "acus diff -p | cut -c1-100 | head -40",
                "acus diff -p --head 40 | cut -c1-100",
            ),
        ] {
            assert_eq!(rw(cmd).as_deref(), Some(want), "{cmd}");
        }
    }

    #[test]
    fn leaves_other_pipes_alone() {
        for cmd in [
            "acus show a.rs --json | head -5",
            "acus show a.rs --head 3 | head -5",
            "acus show a.rs | head -5 > out.txt",
            "acus show a.rs | tail -5",
            "acus find x | head -5 | wc -l",
            "acus show a.rs > o.txt | head -5",
            "acus patch | head -5",
            "acus find --help | head -80",
        ] {
            let got = rw(cmd);
            assert!(
                got.as_deref().is_none_or(|g| !g.contains("--head")),
                "{cmd}: {got:?}"
            );
        }
    }
}
