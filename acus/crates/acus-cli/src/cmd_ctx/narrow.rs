// SPDX-License-Identifier: MPL-2.0
//! `--grep` keeps the matching lines and `--tail` the last ones, plus every failure block.

use regex::Regex;
use std::sync::LazyLock;

/// Lines starting an error or failure: compiler errors, failed tests, panics, tracebacks.
#[expect(clippy::unwrap_used, reason = "a constant pattern")]
static FAILURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(error(\[\w+\])?[:\s]|error TS\d+|Error:|FAILED\b|FAIL\b|failures:|--- FAIL|---- .+ ----$|thread '.*' panicked|Traceback \(most recent call last\)|AssertionError|not ok |Test Case .* failed|[×✗✖] )|\S: error[:\[ ]|panicked at |\btest .* FAILED$",
    )
    .unwrap()
});

/// What `--grep` and `--tail` left of the output.
pub(super) struct Narrowed {
    pub(super) lines: Vec<String>,
    pub(super) grep_dropped: usize,
    pub(super) tail_dropped: usize,
}

/// Keeps the lines matching `re`, or with `context` (`-B`, `-A`) also the lines within that
/// many before and after each match. Groups that do not touch are separated by a `--` line, as
/// grep does. Returns the kept lines and how many were dropped.
fn grep_lines(
    lines: Vec<String>,
    re: &Regex,
    context: Option<(usize, usize)>,
) -> (Vec<String>, usize) {
    let total = lines.len();
    let Some((before, after)) = context else {
        let kept: Vec<String> = lines.into_iter().filter(|l| re.is_match(l)).collect();
        let dropped = total - kept.len();
        return (kept, dropped);
    };
    let mut keep = vec![false; total];
    for (i, l) in lines.iter().enumerate() {
        if re.is_match(l) {
            let start = i.saturating_sub(before);
            let end = i.saturating_add(after).saturating_add(1).min(total);
            keep[start..end].fill(true);
        }
    }
    let mut out = Vec::new();
    let mut prev = None;
    let mut kept = 0;
    for (i, l) in lines.into_iter().enumerate() {
        if !keep[i] {
            continue;
        }
        if prev.is_some_and(|p| p + 1 != i) {
            out.push("--".to_owned());
        }
        out.push(l);
        prev = Some(i);
        kept += 1;
    }
    (out, total - kept)
}

/// `--grep` keeps the matching lines (with `-A`/`-B` the lines around them); `--tail N` then
/// keeps the last N of those and every failure block. A failure block runs from its first
/// line up to
/// the next blank line, at most `max_lines` lines, and at most `max_output` lines of them in
/// all.
/// Each gap becomes a "… K lines dropped" line;
/// blank lines are dropped without being counted.
pub(super) fn narrow(
    lines: Vec<String>,
    (tail, grep, context): (Option<usize>, Option<&Regex>, Option<(usize, usize)>),
    (max_output, max_lines): (usize, usize),
) -> Narrowed {
    let (lines, grep_dropped) = match grep {
        Some(re) => grep_lines(lines, re, context),
        None => (lines, 0),
    };
    let Some(n) = tail else {
        return Narrowed {
            lines,
            grep_dropped,
            tail_dropped: 0,
        };
    };
    let len = lines.len();
    let mut keep = vec![false; len];
    keep[len.saturating_sub(n)..].fill(true);
    let mut budget = max_output;
    let mut i = 0;
    while i < len && budget > 0 {
        if !FAILURE.is_match(&lines[i]) {
            i += 1;
            continue;
        }
        let end = (i + 1..len)
            .find(|&j| lines[j].trim().is_empty())
            .unwrap_or(len)
            .min(i + max_lines.max(1));
        for k in keep.iter_mut().take(end).skip(i) {
            if !*k && budget > 0 {
                *k = true;
                budget -= 1;
            }
        }
        i = end;
    }
    let mut out = Vec::new();
    let mut tail_dropped = 0;
    let mut gap = 0;
    let mut flush = |out: &mut Vec<String>, gap: &mut usize| {
        if *gap > 0 {
            let s = if *gap == 1 { "" } else { "s" };
            out.push(format!("… {} line{s} dropped (--tail {n})", *gap));
            tail_dropped += *gap;
            *gap = 0;
        }
    };
    for (l, k) in lines.into_iter().zip(keep) {
        if k {
            flush(&mut out, &mut gap);
            out.push(l);
        } else if !l.trim().is_empty() && l != "--" {
            // A `--` separator is not a line of its own, so dropping it is not counted.
            gap += 1;
        }
    }
    flush(&mut out, &mut gap);
    Narrowed {
        lines: out,
        grep_dropped,
        tail_dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::narrow;
    use regex::Regex;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_owned).collect()
    }

    #[test]
    fn tail_keeps_the_last_lines_and_every_failure_block() {
        let mut out = String::new();
        for i in 0..30 {
            out += &format!("line {i}\n");
        }
        out += "error[E0425]: cannot find value `x`\n --> src/a.rs:3:5\n  |\n3 | x\n\n";
        for i in 30..40 {
            out += &format!("line {i}\n");
        }
        let n = narrow(lines(&out), (Some(3), None, None), (200, 40));
        assert_eq!(
            n.lines,
            [
                "… 30 lines dropped (--tail 3)",
                "error[E0425]: cannot find value `x`",
                " --> src/a.rs:3:5",
                "  |",
                "3 | x",
                "… 7 lines dropped (--tail 3)",
                "line 37",
                "line 38",
                "line 39",
            ]
        );
        assert_eq!((n.grep_dropped, n.tail_dropped), (0, 37));
        // Short output is untouched, and --tail 0 keeps only the failures.
        assert_eq!(
            narrow(lines("a\nb"), (Some(5), None, None), (200, 40)).lines,
            ["a", "b"]
        );
        let n = narrow(lines("a\nFAILED\n\nb"), (Some(0), None, None), (200, 40));
        assert_eq!(
            n.lines,
            [
                "… 1 line dropped (--tail 0)",
                "FAILED",
                "… 1 line dropped (--tail 0)"
            ]
        );
    }

    #[test]
    fn tail_caps_failure_blocks_at_max_output() {
        let out: String = (0..20).map(|i| format!("error: e{i}\n\n")).collect();
        let n = narrow(lines(&out), (Some(0), None, None), (5, 40));
        assert_eq!(n.lines.iter().filter(|l| l.starts_with("error")).count(), 5);
        assert_eq!(n.tail_dropped, 15);
    }

    #[test]
    fn grep_keeps_matching_lines_then_tail_applies_to_them() {
        let re = Regex::new("^warn").unwrap();
        let l = lines("warn 1\nok\nwarn 2\nok\nwarn 3");
        let n = narrow(l.clone(), (None, Some(&re), None), (200, 40));
        assert_eq!(n.lines, ["warn 1", "warn 2", "warn 3"]);
        assert_eq!((n.grep_dropped, n.tail_dropped), (2, 0));
        let n = narrow(l, (Some(1), Some(&re), None), (200, 40));
        assert_eq!(n.lines, ["… 2 lines dropped (--tail 1)", "warn 3"]);
        assert_eq!((n.grep_dropped, n.tail_dropped), (2, 2));
    }
}
