// SPDX-License-Identifier: MPL-2.0

//! Rewrites and refuses builds and tests piped into filters, with the ctx flags that replace them.

use super::*;

mod chain;
mod ctx;
mod shell;
#[cfg(all(test, any(feature = "bash", feature = "zsh")))]
mod tests;

pub(super) use chain::*;
pub(super) use ctx::*;
pub(super) use shell::*;

/// Refuses a build or test piped into `tail`, `head` or `grep`: the cut output often hides the
/// failure, and the build runs again to see more. `acus ctx '…' | tail` is refused too: ctx has
/// `--tail` and `--grep`. `oseg` and `onext` are the original case of the segments, which the
/// rewritten command quotes.
pub(super) fn filtered_build(
    seg: &Segment,
    next: &Segment,
    rest: &[Segment],
    open: bool,
) -> Option<Verdict> {
    // `rest` holds the filters after `next` in the same chain: a refusal that names a command
    // must cover all of them, or name none. `open` is set when the chain goes on past `rest` into
    // a stage that no ctx flag does: the refusal stands, and names no command.
    let chained = !rest.is_empty();
    let stages: Vec<&str> = std::iter::once(next.orig())
        .chain(rest.iter().map(Segment::orig))
        .collect();
    let filter = fold_filter(seg, next, &stages)?;
    let low = seg.low_unescaped().1;
    let call = call_of(low)?;
    // `timeout N BUILD` and a `gh` log are folded only where the command can be rewritten. Filters
    // that do not fold, or a `$` in the build, keep their existing verdict.
    if call.timed.is_some() || call.build == "gh" {
        let filters: Vec<&str> = std::iter::once(next.low.as_str())
            .chain(rest.iter().map(|s| s.low.as_str()))
            .collect();
        rewrite_build(low, &cut_last(&filters))?;
    }
    let cmd = (seg.orig_unescaped().1.trim())
        .trim_end_matches("2>&1")
        .trim_end();
    if chained {
        if call.is_ctx {
            // Stacked greps that no single ctx flag set expresses run as written; a chain that
            // folds keeps its verdict.
            if stacked_greps(&stages) && ctx_with_flags(cmd, &stages).is_none() {
                return None;
            }
            return Some(Verdict::Deny(
                Rule::CtxPipe,
                chain_ctx_reason(cmd, &stages, filter, open),
            ));
        }
        // Stacked greps that no ctx flag set expresses run as written, as they do after ctx; a
        // chain that folds keeps its verdict.
        if stacked_greps(&stages) && rewrite_build(cmd, &stages).is_none() {
            return None;
        }
        return Some(Verdict::Deny(
            Rule::BuildPipe,
            chain_build_reason(cmd, &stages, call.build, filter, open),
        ));
    }
    let flag = head(next.orig())
        .and_then(|(f, fargs)| ctx_flag(&f.to_lowercase(), &fargs.collect::<Vec<_>>()));
    if call.is_ctx {
        return Some(Verdict::Deny(
            Rule::CtxPipe,
            ctx_pipe_reason(cmd, next, filter, flag),
        ));
    }
    let bare = strip_timeout(cmd).map_or(cmd, |(_, rest)| rest);
    let mut example = if bare.contains('$') {
        ctx_arg(bare).map_or("acus ctx 'COMMAND'".to_owned(), |arg| {
            format!("acus ctx {arg}")
        })
    } else if bare.contains('\'') {
        "acus ctx 'COMMAND'".to_owned()
    } else {
        format!("acus ctx '{bare}'")
    };
    if let Some(secs) = call.timed {
        example += &format!(" --timeout {secs}");
    }
    let reason = match flag {
        Some((flag, what)) => format!(
            "Use `{example} {flag}` instead of piping {build} into {filter}: acus ctx drops build noise, the flag {what}, and the full log is saved for follow-up reads, so the build need not run again. {ESCAPE}\nSend instead: {example} {flag}",
            build = call.build,
        ),
        None => format!(
            "Use acus instead of piping {build} into {filter}: `{example}` drops build noise, shows the failures with the code they point at, and saves the full log for follow-up reads, so the build need not run again (`acus ctx` alone runs the project's tests; `--tail N` keeps the last N lines plus the failures, `--grep RE` keeps matching lines plus their code). {ESCAPE}\nSend instead: {example}",
            build = call.build,
        ),
    };
    Some(Verdict::Deny(Rule::BuildPipe, reason))
}

/// The build or ctx call a segment runs, with `timeout N` taken off.
struct Call<'a> {
    /// The command name: the build's tool, or `acus`.
    build: &'a str,
    /// The `timeout` limit in seconds, when the call has one.
    timed: Option<u64>,
    /// Whether the call is `acus ctx`.
    is_ctx: bool,
}

/// Returns the call when the segment runs a build or `acus ctx` that a filter can fold.
fn call_of(low: &str) -> Option<Call<'_>> {
    // `timeout N BUILD`: the build is what follows; ctx has `--timeout` for the limit.
    let timed = strip_timeout(low);
    let (build, args) = head(timed.map_or(low, |(_, rest)| rest))?;
    let args: Vec<&str> = args.collect();
    let is_ctx = build == "acus" && args.first() == Some(&"ctx");
    let sub = args
        .iter()
        .copied()
        .filter(|a| !a.starts_with(['-', '+']))
        .find(|a| *a != "run");
    // A help or version request prints text, not the output of a build: it passes.
    if args
        .iter()
        .any(|a| matches!(*a, "--help" | "-h" | "--version" | "-V"))
        || sub == Some("help")
    {
        return None;
    }
    let is_build = is_build_command(build, sub, &args);
    if !is_build && !is_ctx {
        return None;
    }
    // Only a build is folded under `timeout`: any other command stays as it is.
    if timed.is_some() && (is_ctx || !is_build) {
        return None;
    }
    Some(Call {
        build,
        timed: timed.map(|(secs, _)| secs),
        is_ctx,
    })
}

/// Returns the filter that a build's output is piped into, when acus folds that pipe.
fn fold_filter<'a>(seg: &Segment, next: &'a Segment, stages: &[&str]) -> Option<&'a str> {
    let (filter, fargs) = head(&next.low).filter(|_| seg.feeds(next))?;
    if !FILTERS.contains(&filter) {
        return None;
    }
    // `sort` and `uniq` dedupe or reorder whole lines, which no ctx flag does: a chain with one
    // runs as written.
    if stages.iter().copied().any(is_sort_or_uniq) {
        return None;
    }
    let fargs: Vec<&str> = fargs.collect();
    // `grep -q` only sets an exit status: no output to cut. `grep -c` prints a count, not the
    // lines.
    if is_quiet(&fargs) || (matches!(filter, "grep" | "egrep" | "rg") && counts(&fargs)) {
        return None;
    }
    Some(filter)
}

/// Reports whether `build` with subcommand `sub` runs a build, a test or a check, or prints a
/// workflow log.
fn is_build_command(build: &str, sub: Option<&str>, args: &[&str]) -> bool {
    match build {
        "cargo" => sub.is_some_and(|s| {
            matches!(
                s,
                "test" | "build" | "check" | "clippy" | "nextest" | "t" | "b" | "c"
            )
        }),
        "swift" => sub.is_some_and(|s| matches!(s, "build" | "test")),
        "go" => sub.is_some_and(|s| matches!(s, "build" | "test" | "vet")),
        "npm" | "pnpm" | "yarn" | "bun" => sub.is_some_and(|s| {
            s.starts_with("test") || s.starts_with("build") || s.contains("check") || s == "lint"
        }),
        "xcodebuild" | "pytest" | "tsc" | "vitest" | "jest" | "mypy" | "make" | "gradle"
        | "gradlew" | "mvn" | "dotnet" => true,
        // The logs of a workflow run, which are as long as a build's.
        "gh" => {
            args.first() == Some(&"run")
                && args.get(1) == Some(&"view")
                && args.iter().any(|a| matches!(*a, "--log" | "--log-failed"))
        }
        _ => false,
    }
}

/// Returns the refusal for `acus ctx` piped into a filter, naming the flags that do its work.
fn ctx_pipe_reason(
    cmd: &str,
    next: &Segment,
    filter: &str,
    flag: Option<(String, String)>,
) -> String {
    // A head that keeps every line the ctx `--tail` keeps does nothing: the command runs as it is.
    let tail_noop = noop_heads_dropped(ctx_tail(cmd), &[next.orig()]).is_empty();
    match ctx_command(cmd).filter(|inner| unquoted_pipe(inner).is_some()) {
        Some(inner) => inner_pipe_reason(inner, filter),
        None if tail_noop => format!(
            "Use `{cmd}` instead of piping acus ctx into {filter}: its `--tail` already keeps every line that {filter} would keep, so the pipe does nothing. {ESCAPE}\nSend instead: {cmd}"
        ),
        None if has_line_count(cmd) => format!(
            "Use `{cmd}` instead of piping acus ctx into {filter}: the command already has a line count (`--tail` or `--max-output`), and a second one would keep other lines. {ESCAPE}\nSend instead: {cmd}"
        ),
        None => {
            // Suggested only where ctx accepts the flag: not when the command has it, or `--`.
            let send = flag
                .as_ref()
                .and_then(|_| ctx_with_flags(cmd, &[next.orig()]));
            let own = format!(
                "Use acus ctx's own flags instead of piping it into {filter}: `acus ctx '…' --tail N` keeps the last N lines plus every failure with the code it points at, `--grep RE` keeps the lines matching RE plus the code they point at, `--max-output N` keeps the first and last N/2 lines. {ESCAPE}"
            );
            match (flag, send) {
                (Some((_, what)), Some(send)) => format!(
                    "Use `{send}` instead of piping acus ctx into {filter}: the flag {what}, so no failure is cut away. {ESCAPE}\nSend instead: {send}"
                ),
                (Some(_), None) => own,
                (None, _) => format!("{own}\nSend instead: {cmd}"),
            }
        }
    }
}

/// Returns `acus ctx 'BUILD' --tail N` for `BUILD | tail -N`, and the same for the other filters
/// `ctx_flag` knows.
pub(super) fn rewrite_build(build: &str, filters: &[&str]) -> Option<String> {
    let (filters, keep) = split_cut(filters);
    let build = build.trim().trim_end_matches("2>&1").trim_end();
    // ctx's shell runs the text again, so a `$` or backtick would expand twice: the suggestion
    // names such a command, nothing rewrites it.
    if build
        .replace("2>&1", "")
        .contains(['<', '>', '\n', '$', '`'])
    {
        return None;
    }
    // The shell ctx runs the text in applies `time` to the build alone: keep it in front.
    // Only a bare `time`: with an option (`time -p`) the text is not just the build.
    let (time, build) = match build.strip_prefix("time ") {
        Some(rest) if rest.trim_start().starts_with('-') => return None,
        Some(rest) => ("time ", rest.trim_start()),
        None => ("", build),
    };
    // `timeout N BUILD` becomes ctx's own `--timeout`, which exits 124 as timeout does.
    let (limit, build) = match strip_timeout(build) {
        Some((secs, rest)) => (format!(" --timeout {secs}"), rest),
        None => (String::new(), build),
    };
    if build.is_empty() {
        return None;
    }
    let filters = noop_heads_dropped(None, filters);
    let flags = ctx_flags(&filters)?;
    Some(format!(
        "{time}acus ctx {}{limit} {}{keep}",
        ctx_arg(build)?,
        flags.join(" ")
    ))
}

/// Returns `acus ctx '…' FLAG` for `acus ctx '…' | tail -N` and the other filters `ctx_flag`
/// knows, or `None` if the ctx call has that flag already.
pub(super) fn rewrite_ctx(ctx: &str, filters: &[&str]) -> Option<String> {
    let (filters, keep) = split_cut(filters);
    Some(format!("{}{keep}", ctx_with_flags(ctx, filters)?))
}

/// Returns the ctx command with `filters` as its flags and the trailing `2>&1` dropped, or `None`
/// where a filter has no exact flag, the command redirects anything but stderr, or it already
/// has a line count.
fn ctx_with_flags(ctx: &str, filters: &[&str]) -> Option<String> {
    let ctx = ctx.trim().trim_end_matches("2>&1").trim_end();
    // A `2>&1` in the quoted command is ctx's own redirect: harmless, so only others stop it.
    if ctx.replace("2>&1", "").contains(['<', '>']) {
        return None;
    }
    // A pipe inside the quoted command would run before the flags: they cannot take its place.
    if ctx_command(ctx).is_some_and(|inner| unquoted_pipe(inner).is_some()) {
        return None;
    }
    // A head that keeps all the lines `--tail` keeps is no filter: the command runs as is.
    let filters = noop_heads_dropped(ctx_tail(ctx), filters);
    if filters.is_empty() {
        return Some(ctx.to_owned());
    }
    let flags = ctx_flags(&filters)?;
    let words = split_words(ctx);
    // `acus ctx … --tail M | tail -N` keeps the smaller count, and the pipe goes.
    if let [flag] = flags.as_slice()
        && let Some(n) = flag.strip_prefix("--tail ")
        && let Some(i) = words.iter().position(|w| *w == "--tail")
    {
        let m: usize = words.get(i + 1)?.parse().ok()?;
        let at = words[i].as_ptr() as usize - ctx.as_ptr() as usize;
        let end = words[i + 1].as_ptr() as usize - ctx.as_ptr() as usize + words[i + 1].len();
        let n: usize = n.parse().ok()?;
        return Some(format!("{}--tail {}{}", &ctx[..at], m.min(n), &ctx[end..]));
    }
    // A line count the command already has cuts what a second one, or any other flag, would keep.
    if has_line_count(ctx) {
        return None;
    }
    for flag in &flags {
        let name = flag.split(' ').next()?;
        if words.iter().any(|w| {
            *w == "--"
                || *w == name
                || w.starts_with(&format!("{name}="))
                || (flag.contains(" -")
                    && ["-A", "-B", "-C", "--after", "--before"]
                        .iter()
                        .any(|p| w.starts_with(p)))
        }) {
            return None;
        }
    }
    Some(format!("{ctx} {}", flags.join(" ")))
}
