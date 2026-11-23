// SPDX-License-Identifier: MPL-2.0
//! Clips lines to a char budget, and the footer that names the rest of a cut listing.

/// Default cap on a printed line in chars; `find --cut N` sets another.
pub const CUT: usize = 200;

/// Trims leading whitespace and clips a hit line to `max` chars around its match (byte offset
/// `col`).
pub fn hit_text(s: &str, col: usize, max: usize) -> String {
    let t = s.trim_start();
    clip(t, col.saturating_sub(s.len() - t.len()), max)
}

/// Cap a line at `max` chars, keeping the window around byte offset `col`, so a minified file
/// costs a few hundred bytes per hit instead of the whole line.
pub fn clip(t: &str, col: usize, max: usize) -> String {
    if t.chars().nth(max).is_none() {
        return t.to_owned();
    }
    let at = t.get(..col).map_or(0, |p| p.chars().count());
    // Up to 60 chars before the match, fewer when the cap is small, so the match stays in view.
    let start = at.saturating_sub(60.min(max / 2));
    let body: String = t.chars().skip(start).take(max).collect();
    let rest = t.chars().count() - start - body.chars().count();
    let head = if start > 0 { "…" } else { "" };
    let tail = if rest > 0 {
        format!("… (+{rest} chars)")
    } else {
        String::new()
    };
    format!("{head}{body}{tail}")
}

/// Caps `text` so it fits in `budget` chars including its newline, with `clip`'s marker; empty
/// when not even the marker fits.
pub fn fit_line(text: &str, budget: usize) -> String {
    let room = budget.saturating_sub(1);
    let total = text.chars().count();
    if total <= room {
        return text.to_owned();
    }
    let mut max = room.saturating_sub(12);
    loop {
        let clipped = clip(text, 0, max);
        if clipped.chars().count() <= room {
            return clipped;
        }
        if max == 0 {
            return String::new();
        }
        max -= 1;
    }
}

/// Clips each line of `body` to `max` chars with `clip`, keeping the newlines.
pub fn clip_lines(body: &str, max: usize) -> String {
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        match line.strip_suffix('\n') {
            Some(text) => {
                out.push_str(&clip(text, 0, max));
                out.push('\n');
            }
            None => out.push_str(&clip(line, 0, max)),
        }
    }
    out
}

/// Keeps the longest prefix of whole lines that fits in `max` chars and counts the lines it leaves
/// out.
pub fn fit_lines(body: &str, max: usize) -> (&str, usize) {
    let (mut used, mut end, mut dropped, mut fits) = (0, 0, 0, true);
    for line in body.split_inclusive('\n') {
        let n = line.chars().count();
        if fits && used + n <= max {
            used += n;
            end += line.len();
        } else {
            fits = false;
            dropped += 1;
        }
    }
    (body.get(..end).unwrap_or_default(), dropped)
}

/// The footer under a truncated listing, naming the command that shows the rest.
pub fn more_lines(path: &str, from: usize, to: usize, n: usize) -> String {
    format!("… {n} more lines (acus show {path}:{from}-{to})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_lines_names_the_rest() {
        assert_eq!(
            more_lines("a.rs", 401, 900, 500),
            "… 500 more lines (acus show a.rs:401-900)"
        );
    }

    #[test]
    fn fit_line_keeps_the_budget_and_marks_the_cut() {
        let line = "x".repeat(30_000);
        let fitted = fit_line(&line, 100);
        let length = fitted.chars().count();
        assert!(length <= 100, "{length}");
        assert!(fitted.contains("(+"), "{fitted}");
        assert_eq!(fit_line("short", 100), "short");
        assert!(fit_line(&line, 8).is_empty());
    }

    #[test]
    fn fits_whole_lines_and_counts_the_rest() {
        let (kept, dropped) = fit_lines("aaa\nbb\nc\n", 8);
        assert_eq!((kept, dropped), ("aaa\nbb\n", 1));
        let (kept, dropped) = fit_lines("long line\nshort\n", 4);
        assert_eq!((kept, dropped), ("", 2));
    }

    #[test]
    fn clip_lines_clips_each_line_and_keeps_newlines() {
        assert_eq!(clip_lines("abcd\nef", 2), "ab… (+2 chars)\nef");
    }
}
