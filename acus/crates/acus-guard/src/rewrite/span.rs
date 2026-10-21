// SPDX-License-Identifier: MPL-2.0

//! Folds reads of a line range or a head and tail window into `acus show`.

use super::*;

/// Returns the line span of a read as the `F:A-B` address `acus find` takes: `A-B`, or `A-` to the
/// end.
pub(super) fn span_addr(from: usize, to: Option<usize>) -> String {
    format!("{from}-{}", to.map(|b| b.to_string()).unwrap_or_default())
}

/// Returns the `head -n N FILE` that says what `cat FILE | head -N` reads, the first lines of
/// FILE, when the guard would refuse that read.
pub(super) fn cat_head_read(segs: &[Segment], i: usize, scope: Option<&Scope>) -> Option<String> {
    let next = segs
        .get(i + 1)
        .filter(|n| segs[i].feeds(n) && !n.opaque() && !continues(segs, i + 1))?;
    let plain = segs[i].command_part(segs[i].low_unescaped().1);
    let read = cat_head(plain, &next.low)?;
    let refused = rule(plain, segs[i].piped(), false, scope, false)?;
    (refused.denied() == Some(Rule::ReadShell)).then_some(read)
}

/// Folds a read or search whose output a display pipe shows, when acus can show the same lines:
/// its acus call piped into the filters that keep them verbatim (`head N`, `tail N`,
/// `cut -c RANGE`). acus prints what cat and grep print, with headers, so a content filter
/// (`sort`, `grep`, `wc`, …) or a redirect at the end refuses as before.
pub(super) fn shown_read(
    segs: &[Segment],
    i: usize,
    rule: Rule,
    plain: &str,
    scope: Option<&Scope>,
) -> Option<Step> {
    if !window_in_file(plain, scope) {
        return None;
    }
    let mut last = i;
    while segs
        .get(last + 1)
        .is_some_and(|n| segs[last].feeds(n) && kept_filter(n))
    {
        last += 1;
    }
    // The chain must end at a filter that is kept, and its output must reach the agent.
    if last == i
        || segs[last].out != Out::Agent
        || segs.get(last + 1).is_some_and(|n| segs[last].feeds(n))
    {
        return None;
    }
    let mut new = rewrite_read(rule, plain, scope)?;
    for s in &segs[i + 1..=last] {
        new.push_str(" | ");
        new.push_str(&s.orig);
    }
    Some(Step::Fold(Fold {
        last,
        rule,
        new,
        from_command: true,
    }))
}

/// Reports whether a display stage can take acus output as it is: `head` or `tail` with a line
/// count (`tail +N` and `tail -f` are other reads) and `cut -c RANGE`.
pub(super) fn kept_filter(s: &Segment) -> bool {
    if s.opaque() {
        return false;
    }
    let range = |r: &str| {
        !r.is_empty()
            && r.bytes()
                .all(|b| b.is_ascii_digit() || b == b',' || b == b'-')
    };
    match split_words(&s.low).as_slice() {
        ["head" | "tail", opts @ ..] => !opts.is_empty() && head_count(opts).is_some(),
        ["cut", "-c", r] => range(r),
        ["cut", o] => o.strip_prefix("-c").is_some_and(range),
        _ => false,
    }
}

/// Reports whether the line window a read names lies in its file. `acus show` rejects a line past
/// the end, which `span_read` avoids by clamping; a window it cannot clamp (no file to count) is
/// kept.
pub(super) fn window_in_file(plain: &str, scope: Option<&Scope>) -> bool {
    let window = match split_words(plain).as_slice() {
        ["sed", "-n", script, file] => sed_span(script).map(|(a, b)| (*file, a, b)),
        ["head", args @ ..] => line_count_args(args).map(|(n, f)| (f, 1, Some(n))),
        ["tail", opts @ ..] => tail_from(opts).map(|(n, f)| (f, n, None)),
        _ => return true,
    };
    let Some((file, from, to)) = window else {
        return true;
    };
    clamp_window(scope, file, from, to).is_some_and(|(_, t)| t == to)
}

/// Returns the lines a `sed -n` script of one range prints: `A,Bp`, `Np`, or `N,$p` (to the end,
/// `None`).
pub(super) fn sed_span(script: &str) -> Option<(usize, Option<usize>)> {
    match sed_lines(unquote(script)).as_deref() {
        Some([(a, b)]) => Some((*a, Some(*b))),
        _ => sed_from(script).map(|n| (n, None)),
    }
}

/// Returns the lines `head -M` (or `tail -M`) prints of the lines `from..=to` (`to` `None`: to the
/// end). `tail` needs an end. Checked arithmetic: a count that overflows is not folded.
pub(super) fn line_window(
    from: usize,
    to: Option<usize>,
    m: usize,
    is_tail: bool,
) -> Option<(usize, usize)> {
    if is_tail {
        let b = to?;
        // The last M lines end at B and start at B-M+1, but not before FROM.
        let start = b
            .checked_add(1)?
            .checked_sub(m)
            .map_or(from, |s| s.max(from));
        Some((start, b))
    } else {
        let end = from.checked_add(m)?.checked_sub(1)?;
        Some((from, to.map_or(end, |b| b.min(end))))
    }
}

/// Returns the window clamped to the last line of FILE. A window that starts past it has no lines,
/// so it is not folded; a file that cannot be read keeps its window as written.
pub(super) fn clamp_window(
    scope: Option<&Scope>,
    file: &str,
    from: usize,
    to: Option<usize>,
) -> Option<(usize, Option<usize>)> {
    let Some(total) = file_lines(scope, file) else {
        return Some((from, to));
    };
    (from <= total).then(|| (from, to.map(|b| b.min(total))))
}

/// Folds a line range that `head -M` or `tail -M` cuts, or that a `cat FILE | sed -n` reads, into
/// the `sed -n` of the lines the pipeline prints: `sed -n 'A,Bp' FILE`, `sed -n 'N,$p' FILE`,
/// `sed -n 'Np' FILE`, `head -N FILE`, and `cat FILE | head -N` or `cat FILE | sed -n …`,
/// where the file is read from stdin. Returns it with the index of the last segment it replaces.
pub(super) fn span_read(
    segs: &[Segment],
    i: usize,
    scope: Option<&Scope>,
) -> Option<(String, usize)> {
    let seg = &segs[i];
    let full = seg.command_part(seg.low_unescaped().1);
    let (plain, tail) = stderr_tail(full.trim());
    let words = split_words(plain);
    // The file, the line range it is read in, the index of the stage that reads it, and whether
    // that stage is a `sed` after `cat FILE`: such a read is one on its own when nothing follows.
    let (file, (from, to), read_at, cat_sed) = match words.as_slice() {
        ["sed", "-n", script, file] => (*file, sed_span(script)?, i, false),
        ["head", args @ ..] => {
            let (n, file) = line_count_args(args)?;
            (file, (1, Some(n)), i, false)
        }
        ["cat", file] if !file.starts_with('-') => {
            let next = segs
                .get(i + 1)
                .filter(|n| seg.feeds(n) && !n.opaque() && !feeds_other(segs, i + 1))?;
            match split_words(&next.low).as_slice() {
                ["sed", "-n", script] => (*file, sed_span(script)?, i + 1, true),
                ["head", args @ ..] => (*file, (1, Some(head_count(args)?)), i + 1, false),
                _ => return None,
            }
        }
        _ => return None,
    };
    if !file_operand(file, scope) {
        return None;
    }
    // The stage that reads FILE must be a read the guard refuses, as for `cat FILE | head`.
    if rule(full, seg.piped(), false, scope, false)?.denied() != Some(Rule::ReadShell) {
        return None;
    }
    // The window ends at the file's last line: `acus show` fails on a line past the end.
    let (from, to) = clamp_window(scope, file, from, to)?;
    // The stage after the read is a `head` or `tail` line count that ends the pipeline.
    let filter = segs
        .get(read_at + 1)
        .filter(|n| segs[read_at].feeds(n) && !n.opaque() && !continues(segs, read_at + 1));
    let (a, b, last) = match filter {
        Some(n) => {
            let w = split_words(&n.low);
            let (is_tail, opts) = match w.as_slice() {
                ["head", opts @ ..] => (false, opts),
                ["tail", opts @ ..] => (true, opts),
                _ => return None,
            };
            let (a, b) = line_window(from, to, head_count(opts)?, is_tail)?;
            (a, Some(b), read_at + 1)
        }
        None if cat_sed && !continues(segs, read_at) => (from, to, read_at),
        None => return None,
    };
    let read = match b {
        Some(b) => format!("sed -n '{a},{b}p' {file}{tail}"),
        None => format!("sed -n '{a},$p' {file}{tail}"),
    };
    Some((read, last))
}

/// Returns the `Limited` search of `segs[i]` when it is a refused search piped into `head` or
/// `tail`.
pub(crate) fn limited_search(segs: &[Segment], i: usize, scope: Option<&Scope>) -> Option<Limited> {
    let seg = segs.get(i)?;
    let (escaped, plain) = seg.low_unescaped();
    if escaped || seg.opaque() {
        return None;
    }
    let plain = seg.command_part(plain);
    let (stripped, tail) = stderr_tail(plain.trim());
    let filter = segs
        .get(i + 1)
        .filter(|n| seg.feeds(n) && !n.opaque() && !continues(segs, i + 1))?;
    let w = split_words(&filter.low);
    let (name, opts) = match w.as_slice() {
        ["head", opts @ ..] => ("head", opts),
        ["tail", opts @ ..] => ("tail", opts),
        _ => return None,
    };
    let count = head_count(opts)?;
    let (cmd, words) = head(stripped)?;
    let cmd = cmd.to_lowercase();
    if !GREPS.contains(&cmd.as_str())
        || rule(plain, seg.piped(), false, scope, false)?.denied() != Some(Rule::Grep)
    {
        return None;
    }
    let args: Vec<&str> = words.collect();
    let s = parse_search(&cmd, &args);
    if s.inexact || is_count(&s) {
        return None;
    }
    let listed = s.flags.iter().any(|f| f == "-l");
    let per_match = s.flags.iter().any(|f| f == "-o");
    let context = s
        .flags
        .iter()
        .any(|f| matches!(f.split(' ').next(), Some("-A" | "-B" | "-C")));
    // `--last` counts the lines of each file, so it needs the one regular file searched.
    let one_file = matches!(
        s.operands.as_slice(),
        [file] if scope.is_some_and(|sc| sc.is_file(file))
    );
    let limit = match name {
        "head" if !listed && !per_match => Some(format!("--max-hits {count}")),
        "tail" if !listed && !context && !per_match && one_file => Some(format!("--last {count}")),
        _ => None,
    };
    Some(Limited {
        find: find_cmd(&s),
        exact: limit.is_some() && !context,
        limit,
        last: i + 1,
        tail,
    })
}
