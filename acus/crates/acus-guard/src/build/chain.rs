// SPDX-License-Identifier: MPL-2.0

//! Owns the filter chains after a build and the reasons that refuse them.

use super::*;

/// Lists the commands that only cut or select the lines of the output they read.
pub(crate) const FILTERS: &[&str] = &[
    "tail",
    "head",
    "grep",
    "egrep",
    "rg",
    "select-object",
    "select",
    "select-string",
    "sls",
];

/// Returns the last stage of the filter chain after the build at `i`. Each stage reads the one
/// before it and is a filter, or a `sort` or `uniq`; a chain with one of the latter runs as
/// written.
/// Returns the refusal of an acus ctx pipe whose filter is followed by more stages. A command is
/// named only when every stage maps to a ctx flag: any other command would drop the stages after
/// the pair.
pub(crate) fn chain_ctx_reason(cmd: &str, stages: &[&str], filter: &str, open: bool) -> String {
    let send = if open {
        None
    } else {
        rewrite_ctx(cmd, &cut_last(stages))
    };
    match send {
        Some(send) => format!(
            "Use `{send}` instead of piping acus ctx into {filter}: every filter after it maps to a ctx flag, so no failure is cut away. {ESCAPE}\nSend instead: {send}"
        ),
        None => format!(
            "Use acus ctx's own flags instead of piping it into {filter}: {}, so no command is suggested. {ESCAPE}",
            blockers(stages)
        ),
    }
}

/// Returns the refusal of a build pipe whose filter is followed by more stages, as
/// `chain_ctx_reason`.
pub(crate) fn chain_build_reason(
    cmd: &str,
    stages: &[&str],
    build: &str,
    filter: &str,
    open: bool,
) -> String {
    let send = if open {
        None
    } else {
        rewrite_build(cmd, &cut_last(stages))
    };
    match send {
        Some(send) => format!(
            "Use `{send}` instead of piping {build} into {filter}: every filter after it maps to a ctx flag, so the build runs once and no failure is cut away. {ESCAPE}\nSend instead: {send}"
        ),
        None => format!(
            "Use acus instead of piping {build} into {filter}: {}, so no command is suggested. {ESCAPE}",
            blockers(stages)
        ),
    }
}

/// Orders the stages as their Send line runs them. A `cut` followed only by `head` or `tail`
/// moves behind them: a cut clips each line and a window picks lines, in either order. A `cut`
/// before a grep stays where it is, since the grep would then match the clipped lines.
pub(crate) fn cut_last<'a>(stages: &[&'a str]) -> Vec<&'a str> {
    let mut out = stages.to_vec();
    let Some(at) = stages
        .iter()
        .position(|s| filter_name(s).as_deref() == Some("cut"))
    else {
        return out;
    };
    let after = &stages[at + 1..];
    if !after.is_empty()
        && after
            .iter()
            .all(|s| matches!(filter_name(s).as_deref(), Some("head" | "tail")))
    {
        let cut = out.remove(at);
        out.push(cut);
    }
    out
}

/// Returns what blocks a chain's ctx form, for a refusal that suggests no command.
pub(crate) fn blockers(stages: &[&str]) -> String {
    let names = blocking_names(stages);
    if names.is_empty() {
        return "the chain has no exact ctx form".to_owned();
    }
    let list: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    format!("no ctx flag does {}", list.join(", "))
}

/// Returns the filters of a chain that no ctx flag does, each once and in order. A trailing
/// `cut` is left out: it stays behind the rewritten command.
pub(crate) fn blocking_names(stages: &[&str]) -> Vec<String> {
    let (filters, _) = split_cut(stages);
    let mut names: Vec<String> = Vec::new();
    for &stage in filters {
        let Some((name, args)) = head(stage) else {
            continue;
        };
        let name = name.to_lowercase();
        let args: Vec<&str> = args.collect();
        let blocks = ctx_flag(&name, &args).is_none();
        if blocks && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Returns the `--tail N` count of an acus ctx command.
pub(crate) fn ctx_tail(ctx: &str) -> Option<usize> {
    let words = split_words(ctx);
    let at = words.iter().position(|w| *w == "--tail")?;
    words.get(at + 1)?.parse().ok()
}

/// Returns the window of a `head` or `tail` stage, or `None` for any other stage.
pub(crate) fn tail_or_head(stage: &str) -> Option<Window> {
    let (name, args) = head(stage)?;
    let name = name.to_lowercase();
    if !matches!(name.as_str(), "head" | "tail") {
        return None;
    }
    let args: Vec<&str> = args.collect();
    window_min(&name, &args, 0)
}

/// Returns the filters without each `head -N` that keeps every line the stage before it kept.
/// `tail` is the `--tail T` of the ctx command or a `tail -T` stage, and with N >= T the head
/// changes nothing.
pub(crate) fn noop_heads_dropped<'a>(mut tail: Option<usize>, filters: &[&'a str]) -> Vec<&'a str> {
    let mut kept = Vec::new();
    for &stage in filters {
        let window = tail_or_head(stage);
        if matches!((&window, tail), (Some(Window::First(n)), Some(t)) if *n >= t) {
            continue;
        }
        tail = match window {
            Some(Window::Last(n)) => Some(n),
            _ => None,
        };
        kept.push(stage);
    }
    kept
}

/// Reports whether a stage is a `sort` or `uniq`, which ctx cannot fold: it dedupes or reorders
/// whole lines.
pub(crate) fn is_sort_or_uniq(stage: &str) -> bool {
    filter_name(stage).is_some_and(|f| matches!(f.as_str(), "sort" | "uniq"))
}

/// Reports whether every stage is a `grep` or `egrep` without context lines: stacked like that,
/// no single ctx flag set expresses them. A context window keeps its refusal, since before a
/// `grep -v` it reads other lines.
pub(crate) fn stacked_greps(stages: &[&str]) -> bool {
    stages.iter().copied().all(|stage| {
        let Some((name, args)) = head(stage) else {
            return false;
        };
        let name = name.to_lowercase();
        let args: Vec<&str> = args.collect();
        matches!(name.as_str(), "grep" | "egrep")
            && ctx_flag(&name, &args).is_some_and(|(flag, _)| !has_context(&flag))
    })
}

pub(crate) fn filter_last(segs: &[Segment], i: usize) -> usize {
    let mut last = i;
    // A display command such as `cut` counts in the middle too: a `cut` before a `tail` stays in
    // the chain, so no Send line leaves it out. A bare `cat` does not, since `task_log` folds
    // `cat FILE | cat`.
    while segs.get(last + 1).is_some_and(|n| {
        let display =
            filter_name(n.low_unescaped().1).is_some_and(|f| DISPLAY.contains(&f.as_str()));
        !n.opaque() && segs[last].feeds(n) && (is_chain_stage(n) || display)
    }) {
        last += 1;
    }
    last
}

pub(crate) fn is_chain_stage(s: &Segment) -> bool {
    filter_name(s.low_unescaped().1)
        .is_some_and(|f| FILTERS.contains(&f.as_str()) || matches!(f.as_str(), "sort" | "uniq"))
}

/// Reports whether the pipe segment `last` feeds into a command that is not an output filter.
/// Then the output does not reach the model as it is, and nothing is converted.
pub(crate) fn feeds_other(segs: &[Segment], last: usize) -> bool {
    segs.get(last + 1)
        .is_some_and(|s| segs[last].feeds(s) && !display_stage(s))
}

/// Lists the commands that reshape or page the lines they read without running anything: a pipe
/// into one of them shows the agent what the stage before it printed. `wc` is not one: it prints a
/// count, not the lines, so `grep X | wc -l` runs as written.
pub(crate) const DISPLAY: &[&str] = &[
    "sort", "uniq", "cut", "column", "tr", "nl", "fold", "less", "more",
];

/// Reports whether a stage only shows or cuts what it reads: an output filter, a display command,
/// or a `cat` without arguments.
pub(crate) fn display_stage(s: &Segment) -> bool {
    let Some((h, mut args)) = head(s.low_unescaped().1) else {
        return false;
    };
    let h = h.to_lowercase();
    if h == "cat" {
        return args.next().is_none();
    }
    FILTERS.contains(&h.as_str()) || DISPLAY.contains(&h.as_str())
}

/// Reports whether the pipe that starts at segment `i` runs through display stages only and ends
/// in the agent's output. Then the first stage's output is what the agent reads, so it is judged
/// as such.
pub(crate) fn shown_by_filters(segs: &[Segment], i: usize) -> bool {
    if !segs.get(i + 1).is_some_and(|n| segs[i].feeds(n)) {
        return false;
    }
    let mut j = i + 1;
    loop {
        if !display_stage(&segs[j]) {
            return false;
        }
        match segs.get(j + 1) {
            Some(next) if segs[j].feeds(next) => j += 1,
            _ => return segs[j].out == Out::Agent,
        }
    }
}
