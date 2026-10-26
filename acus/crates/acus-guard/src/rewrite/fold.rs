// SPDX-License-Identifier: MPL-2.0

//! Folds the commands that a rewrite can turn into one acus call.

use super::*;

/// Returns what to do at segment `i`, or `None` if the whole command must be refused instead.
pub(super) fn fold_at(segs: &[Segment], i: usize, scope: Option<&Scope>) -> Option<Step> {
    let seg = &segs[i];
    let (escaped, plain) = seg.low_unescaped();
    if escaped || seg.opaque() {
        return Some(Step::Keep);
    }
    let full = plain;
    let plain = seg.command_part(plain);
    let pair = segs
        .get(i + 1)
        .filter(|n| !n.opaque() && seg.feeds(n) && !feeds_other(segs, i + 1))
        .and_then(|next| {
            filtered_build(
                seg,
                next,
                &segs[i + 2..=filter_last(segs, i).max(i + 1)],
                continues(segs, filter_last(segs, i)),
            )
        });
    if let Some(pair) = pair {
        let last = filter_last(segs, i);
        let filters: Vec<&str> = segs[i + 1..=last].iter().map(|s| s.low.as_str()).collect();
        // The folded call must be the last stage: nothing may read its output.
        if continues(segs, last) {
            return None;
        }
        let (rule, new) = match pair.denied() {
            Some(rule @ Rule::BuildPipe) => (rule, rewrite_build(full, &filters)?),
            Some(rule @ Rule::CtxPipe) => (rule, rewrite_ctx(full, &filters)?),
            _ => return None,
        };
        return Some(Step::Fold(Fold {
            last,
            rule,
            new,
            from_command: false,
        }));
    }
    if let Some((last, new)) = acus_pipe::fold(segs, i) {
        return Some(Step::Fold(Fold {
            last,
            rule: Rule::AcusPipe,
            new,
            from_command: false,
        }));
    }
    if let Some((last, new)) = task_log::fold(segs, i) {
        return Some(Step::Fold(Fold {
            last,
            rule: Rule::TaskLog,
            new,
            from_command: false,
        }));
    }
    if let Some(fold) = read_search(segs, i, scope) {
        return Some(Step::Fold(fold));
    }
    if let Some((read, last)) = tail_head_read(segs, i, scope) {
        let new = rewrite_read(Rule::ReadShell, &read, scope)?;
        return Some(Step::Fold(Fold {
            last,
            rule: Rule::ReadShell,
            new,
            from_command: true,
        }));
    }
    if let Some((read, last)) = span_read(segs, i, scope) {
        let new = rewrite_read(Rule::ReadShell, &read, scope)?;
        return Some(Step::Fold(Fold {
            last,
            rule: Rule::ReadShell,
            new,
            from_command: true,
        }));
    }
    if let Some(read) = cat_head_read(segs, i, scope) {
        let new = rewrite_read(Rule::ReadShell, &read, scope)?;
        return Some(Step::Fold(Fold {
            last: i + 1,
            rule: Rule::ReadShell,
            new,
            from_command: true,
        }));
    }
    // `grep … | head -N` and `grep … | tail -N` are the converted search with its limit.
    if let Some(Limited {
        find,
        limit: Some(limit),
        exact: true,
        last,
        tail,
    }) = limited_search(segs, i, scope)
    {
        return Some(Step::Fold(Fold {
            last,
            rule: Rule::Grep,
            new: format!("{find} {limit}{tail}"),
            from_command: true,
        }));
    }
    fold_plain(segs, i, scope, plain)
}

/// Folds or refuses a command that no build, read or log fold claimed.
pub(super) fn fold_plain(
    segs: &[Segment],
    i: usize,
    scope: Option<&Scope>,
    plain: &str,
) -> Option<Step> {
    let seg = &segs[i];
    let feeds = segs.get(i + 1).is_some_and(|n| seg.feeds(n));
    // `find -name` is not refused, but `acus files` lists the same files. It exits 1 when
    // nothing matches where `find` exits 0, so a listing that gates `&&`/`||` stays as it is.
    if !seg.piped()
        && !feeds
        && !seg.gates
        && let Some(new) = rewrite_read(Rule::GlobTool, plain, scope)
    {
        return Some(Step::Fold(Fold {
            last: i,
            rule: Rule::GlobTool,
            new,
            from_command: true,
        }));
    }
    // `git diff` whose output the agent reads becomes `acus diff`, which groups it by function.
    if seg.out == Out::Agent
        && !seg.piped()
        && !seg.gates
        && let Some(new) = git::diff_acus(plain)
    {
        return Some(Step::Fold(Fold {
            last: i,
            rule: Rule::GitDiff,
            new,
            from_command: true,
        }));
    }
    // A search or read whose output a display stage shows is refused, never rewritten into a pipe
    // to acus: only `cat FILE | head N` and the build filters have an exact equivalent.
    let shown = shown_by_filters(segs, i);
    let Some(refused) = rule(plain, seg.piped(), false, scope, feeds && !shown) else {
        return Some(Step::Keep);
    };
    match refused.denied() {
        Some(Rule::Grep | Rule::ReadShell) if seg.gates && lists_files(plain) => None,
        Some(rule @ (Rule::Grep | Rule::ReadShell)) if shown => {
            shown_read(segs, i, rule, plain, scope)
        }
        Some(rule @ (Rule::Grep | Rule::ReadShell)) => Some(Step::Fold(Fold {
            last: i,
            rule,
            new: rewrite_read(rule, plain, scope)?,
            from_command: true,
        })),
        Some(_) => None,
        None => Some(Step::Keep),
    }
}

/// Rewrites the `echo` after a folded pipeline (`…; echo "exit=${PIPESTATUS[0]}"`) with its status
/// reference (`${PIPESTATUS[0]}` or zsh `${pipestatus[1]}`) replaced by `$?`. The acus call that
/// replaced the pipeline exits with the command's status, so `$?` is the status the pipeline had.
pub(super) fn exit_status_echo(cmd: &str, segs: &[Segment], i: usize) -> Option<String> {
    let seg = segs.get(i)?;
    let prev = segs.get(i.checked_sub(1)?)?;
    let gap = cmd.get(prev.range.end..seg.range.start)?;
    if gap.trim() != ";"
        || seg.out != Out::Agent
        || !head(&seg.low).is_some_and(|(h, _)| h == "echo")
    {
        return None;
    }
    let text = cmd.get(seg.range.clone())?;
    // Inside single quotes the reference is literal text, not an expansion.
    if text.contains('\'') {
        return None;
    }
    let new = text
        .replace("${PIPESTATUS[0]}", "$?")
        .replace("${pipestatus[1]}", "$?");
    // Any other status reference is left for the agent to fix, so the echo is not touched.
    (new != text && !new.to_lowercase().contains("pipestatus")).then_some(new)
}

/// Returns the `sed -n 'N,Mp' FILE` that says what `tail -n +N FILE | head -M` and
/// `cat FILE | tail -n +N | head -M` read: lines N to N+M-1 of FILE, with the index of the last
/// segment it replaces.
pub(super) fn tail_head_read(
    segs: &[Segment],
    i: usize,
    scope: Option<&Scope>,
) -> Option<(String, usize)> {
    let seg = &segs[i];
    let full = seg.command_part(seg.low_unescaped().1);
    let (plain, tail) = stderr_tail(full.trim());
    let words = split_words(plain);
    // The tail stage is this segment, or the one after a `cat FILE`.
    let (from, file, tail_at) = match words.as_slice() {
        ["tail", opts @ ..] => {
            let (from, file) = tail_from(opts)?;
            (from, file, i)
        }
        ["cat", file] if !file.starts_with('-') => {
            let next = segs
                .get(i + 1)
                .filter(|n| seg.feeds(n) && !n.opaque() && !feeds_other(segs, i + 1))?;
            let tw = split_words(next.low_unescaped().1);
            let ["tail", opts @ ..] = tw.as_slice() else {
                return None;
            };
            (from_line(opts)?, *file, i + 1)
        }
        _ => return None,
    };
    let head_at = tail_at + 1;
    let head = segs
        .get(head_at)
        .filter(|n| segs[tail_at].feeds(n) && !n.opaque() && !feeds_other(segs, head_at))?;
    let hw = split_words(&head.low);
    let ["head", opts @ ..] = hw.as_slice() else {
        return None;
    };
    let count = head_count(opts)?;
    if continues(segs, head_at) {
        return None;
    }
    // The first stage must be a read the guard refuses, as for `cat FILE | head`.
    let refused = rule(full, seg.piped(), false, scope, false)?;
    if refused.denied() != Some(Rule::ReadShell) {
        return None;
    }
    let to = from.checked_add(count)? - 1;
    // The window ends at the file's last line, as `span_read` does: `acus show` rejects past it.
    let (from, to) = clamp_window(scope, file, from, Some(to))?;
    let to = to?;
    Some((format!("sed -n '{from},{to}p' {file}{tail}"), head_at))
}

/// Folds a read of one file piped into a search: `sed -n 'A,Bp' F` (or `'N,$p'`), `head -N F`,
/// `tail -N F` or `cat F` piped into `grep PAT` searches the lines of that range in one
/// `acus find PAT 'F:A-B'` (`F:N-`, `F:-N`, or `F` for `cat`). The grep is the last stage, or
/// followed by the `head` or `tail` count that `limited_search` folds too. Only a read the guard
/// refuses is folded.
pub(super) fn read_search(segs: &[Segment], i: usize, scope: Option<&Scope>) -> Option<Fold> {
    let seg = &segs[i];
    let full = seg.command_part(seg.low_unescaped().1);
    let (plain, tail) = stderr_tail(full.trim());
    let (file, addr) = match split_words(plain).as_slice() {
        ["sed", "-n", script, file] => {
            let (a, b) = sed_span(script)?;
            let (a, b) = clamp_window(scope, file, a, b)?;
            (*file, span_addr(a, b))
        }
        ["head", args @ ..] => {
            let (n, file) = line_count_args(args)?;
            let (a, b) = clamp_window(scope, file, 1, Some(n))?;
            (file, span_addr(a, b))
        }
        ["tail", args @ ..] => {
            let (n, file) = line_count_args(args)?;
            (file, format!("-{n}"))
        }
        ["cat", file] if !file.starts_with('-') => (*file, String::new()),
        _ => return None,
    };
    let target = if addr.is_empty() {
        file.to_owned()
    } else {
        format!("{file}:{addr}")
    };
    if !file_operand(file, scope)
        || rule(full, seg.piped(), false, scope, false)?.denied() != Some(Rule::ReadShell)
    {
        return None;
    }
    let g = segs.get(i + 1).filter(|n| seg.feeds(n) && !n.opaque())?;
    let (escaped, gplain) = g.low_unescaped();
    if escaped {
        return None;
    }
    let (stripped, gtail) = stderr_tail(g.command_part(gplain).trim());
    let (cmd, words) = head(stripped)?;
    let cmd = cmd.to_lowercase();
    if !matches!(cmd.as_str(), "grep" | "egrep") {
        return None;
    }
    let args: Vec<&str> = words.collect();
    let mut s = parse_search(&cmd, &args);
    if s.inexact || s.pattern.is_none() || s.recursive || !s.operands.is_empty() || is_count(&s) {
        return None;
    }
    s.operands = vec![target.as_str()];
    let (mut last, mut limit) = (i + 1, String::new());
    if let Some(n) = segs.get(i + 2).filter(|n| g.feeds(n) && !n.opaque()) {
        let w = split_words(n.low_unescaped().1);
        let (is_tail, opts) = match w.as_slice() {
            ["head", opts @ ..] => (false, opts),
            ["tail", opts @ ..] => (true, opts),
            _ => return None,
        };
        let count = head_count(opts)?;
        let listed = s.flags.iter().any(|f| f == "-l");
        let context = s
            .flags
            .iter()
            .any(|f| matches!(f.split(' ').next(), Some("-A" | "-B" | "-C")));
        if listed || context {
            return None;
        }
        limit = if is_tail {
            format!(" --last {count}")
        } else {
            format!(" --max-hits {count}")
        };
        last = i + 2;
    }
    if continues(segs, last) {
        return None;
    }
    let tail = if gtail.is_empty() { tail } else { gtail };
    Some(Fold {
        last,
        rule: Rule::Grep,
        new: format!("{}{limit}{tail}", find_cmd(&s)),
        from_command: true,
    })
}
