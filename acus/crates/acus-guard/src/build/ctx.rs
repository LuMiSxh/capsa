// SPDX-License-Identifier: MPL-2.0

//! Turns the filters of a pipe into the `acus ctx` flags that replace them.

use super::*;

/// Returns the `acus ctx` flag that does what a `tail`, `head`, `grep` or PowerShell `Select-*`
/// filter did, or `None` for options ctx has no equivalent of (`grep -v`, `tail -n +5`).
/// Treats cmdlet switches as case-insensitive: `filter` is lowercased, `args` keep their case.
pub(crate) fn ctx_flag(filter: &str, args: &[&str]) -> Option<(String, String)> {
    match filter {
        "tail" | "head" | "select-object" | "select" => ctx_window_flag(filter, args),
        "grep" => ctx_grep_flag(args),
        // `egrep` is `grep -E`.
        "egrep" => ctx_grep_flag(&[&["-E"], args].concat()),
        "rg" => grep_flag(args, false),
        "select-string" | "sls" => ctx_select_string_flag(args),
        _ => None,
    }
}

/// Returns the flag for a `head`, `tail` or `Select-Object -First/-Last` filter. `tail -n +5`
/// counts from the start, so it has no equivalent.
pub(crate) fn ctx_window_flag(filter: &str, args: &[&str]) -> Option<(String, String)> {
    let (last, n) = if matches!(filter, "select-object" | "select") {
        let mut it = args.iter().copied();
        let mut found = None;
        while let Some(a) = it.next() {
            let last = match a.to_lowercase().as_str() {
                "-first" => false,
                "-last" => true,
                _ => return None,
            };
            found = Some((last, digits(it.next()?).filter(|n| *n >= 1)?));
        }
        found?
    } else {
        match window_min(filter, args, 0)? {
            Window::First(n) => (false, n),
            Window::Last(n) => (true, n),
            Window::From(_) => return None,
        }
    };
    Some(if last {
        (
            format!("--tail {n}"),
            format!("keeps the last {n} lines plus every failure with the code it points at"),
        )
    } else {
        (
            format!("--max-output {}", n.max(2)),
            format!(
                "keeps the first and last {} lines plus the code the failures point at",
                n.max(2) / 2
            ),
        )
    })
}

/// Returns the flag for a `grep` filter (`egrep` comes with `-E` in front).
pub(crate) fn ctx_grep_flag(args: &[&str]) -> Option<(String, String)> {
    grep_flag(args, true)
}

/// Sets `posix` when the pattern is basic or extended syntax (`grep`), not Rust regex (`rg`).
pub(crate) fn grep_flag(args: &[&str], posix: bool) -> Option<(String, String)> {
    // After `--` the words are no options, which the filter was never checked for.
    if args.contains(&"--") {
        return None;
    }
    let (mut pat, mut icase, mut ext, mut drop) = (None, false, false, false);
    let mut around: Vec<String> = Vec::new();
    let context = |c: char, v: &str| Some(format!("-{c} {}", v.parse::<usize>().ok()?));
    for opt in lex_grep(args) {
        match opt {
            GrepOpt::Short('e', Some(v)) if pat.is_none() => pat = Some(v),
            GrepOpt::Short('i', None) | GrepOpt::Long("ignore-case", None) => icase = true,
            GrepOpt::Short('E', None) | GrepOpt::Long("extended-regexp", None) => ext = true,
            GrepOpt::Short('v', None) | GrepOpt::Long("invert-match", None) => drop = true,
            GrepOpt::Short('n', None) | GrepOpt::Long("line-number", None) => {}
            GrepOpt::Short(c @ ('A' | 'B' | 'C'), Some(v)) => around.push(context(c, v)?),
            GrepOpt::Long("after-context", Some(v)) => around.push(context('A', v)?),
            GrepOpt::Long("before-context", Some(v)) => around.push(context('B', v)?),
            GrepOpt::Long("context", Some(v)) => around.push(context('C', v)?),
            GrepOpt::Context(n) => around.push(context('C', n)?),
            GrepOpt::Word(w) if pat.is_none() => pat = Some(w),
            _ => return None,
        }
    }
    grep_flag_text(pat?, icase, ext, drop, &around, posix)
}

/// Returns the flag for a `Select-String` filter, which ignores case unless `-CaseSensitive` is
/// given.
pub(crate) fn ctx_select_string_flag(args: &[&str]) -> Option<(String, String)> {
    let (mut pat, mut icase) = (None, true);
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        let k = a.to_lowercase();
        match k.as_str() {
            "-e" | "-pattern" if pat.is_none() => pat = Some(it.next()?),
            "-casesensitive" => icase = false,
            _ if k.starts_with('-') => return None,
            _ if pat.is_none() => pat = Some(a),
            _ => return None,
        }
    }
    grep_flag_text(pat?, icase, false, false, &[], false)
}

/// Returns the `--grep` or `--drop` flag for a pattern, with the context flags (`-A 2`) that go
/// with it.
pub(crate) fn grep_flag_text(
    pat: &str,
    icase: bool,
    ext: bool,
    drop: bool,
    around: &[String],
    posix: bool,
) -> Option<(String, String)> {
    if !plain_word(pat) {
        return None;
    }
    let mut pat = unquote(pat).to_owned();
    if pat.contains('\'') {
        return None;
    }
    if posix {
        pat = convert::posix_to_rust(&pat, ext)?;
    }
    if icase {
        pat.insert_str(0, "(?i)");
    }
    // `grep -v` drops the matching lines, so a context flag has no meaning for it.
    if drop && !around.is_empty() {
        return None;
    }
    let (name, what) = if drop {
        ("--drop", "drops the lines matching it")
    } else {
        (
            "--grep",
            "keeps the lines matching it plus the code they point at",
        )
    };
    let mut flag = format!("{name} '{pat}'");
    for c in around {
        flag += &format!(" {c}");
    }
    Some((flag, what.to_owned()))
}

/// Returns the `acus ctx` flags for a chain of filters, or `None` if one has no flag or a flag
/// repeats. A `sort` or `uniq` has none: ctx only drops adjacent repeats, so such a chain runs as
/// written.
pub(crate) fn ctx_flags(filters: &[&str]) -> Option<Vec<String>> {
    let mut flags: Vec<String> = Vec::new();
    // Whether a flag before this one carries context lines.
    let mut windowed = false;
    let mut i = 0;
    while i < filters.len() {
        let f = filters[i];
        if has_redirect(f) {
            return None;
        }
        let (name, args) = head(f)?;
        let name = name.to_lowercase();
        // A line count cuts what the flags before it kept, so ctx can only apply it last.
        if matches!(name.as_str(), "head" | "tail" | "select-object" | "select")
            && i + 1 != filters.len()
        {
            return None;
        }
        let (flag, _) = ctx_flag(&name, &args.collect::<Vec<_>>())?;
        let first = flag.split(' ').next()?;
        // Each flag once, except `--drop`, which repeats for every `grep -v`.
        if first != "--drop" && flags.iter().any(|g| g.split(' ').next() == Some(first)) {
            return None;
        }
        // ctx applies `--drop` before `--grep`, so a context window taken before a `grep -v` would
        // cover other lines than the pipe does: that order is refused, the reverse one folds.
        if first == "--drop" && windowed {
            return None;
        }
        windowed |= has_context(&flag);
        flags.push(flag);
        i += 1;
    }
    Some(flags)
}

/// Reports whether a `--grep` flag carries context lines (`-A N`, `-B N`, `-C N`): its pattern is
/// quoted, and the context follows the closing quote.
pub(crate) fn has_context(flag: &str) -> bool {
    flag.rsplit_once('\'')
        .is_some_and(|(_, rest)| rest.contains('-'))
}

/// Returns the lowercase command name of a filter segment.
pub(crate) fn filter_name(seg: &str) -> Option<String> {
    head(seg).map(|(h, _)| h.to_lowercase())
}

/// Quotes a command as the one argument of `acus ctx`, for the suggestion text: single-quoted, or
/// double-quoted when it holds `$`. `rewrite_build` never rewrites a `$` command, since ctx's
/// shell would expand it a second time. Returns `None` where neither is safe: a double quote,
/// backslash
/// or backtick in it, or a `$` in a command that also has a single quote.
pub(crate) fn ctx_arg(cmd: &str) -> Option<String> {
    if !cmd.contains('$') {
        return Some(quote(cmd));
    }
    let unsafe_in_dq = cmd.contains(['"', '\\', '`']) || cmd.contains('\'');
    (!unsafe_in_dq).then(|| format!("\"{cmd}\""))
}

/// Reports whether grep-style `args` count the matching lines (`-c`, `--count`), so the count is
/// what is shown.
pub(crate) fn counts(args: &[&str]) -> bool {
    lex_grep(args)
        .iter()
        .any(|o| matches!(o, GrepOpt::Long("count", _) | GrepOpt::Short('c', _)))
}

/// Returns the command text that `acus ctx 'TEXT' …` runs: its first word that is not a flag,
/// unquoted.
pub(crate) fn ctx_command(ctx: &str) -> Option<&str> {
    let words = split_words(ctx);
    let at = words.iter().position(|w| *w == "ctx")?;
    words[at + 1..]
        .iter()
        .find(|w| !w.starts_with('-'))
        .copied()
        .map(unquote)
}

/// Reports whether the ctx command already has a line count (`--tail` or `--max-output`).
pub(crate) fn has_line_count(ctx: &str) -> bool {
    split_words(ctx)
        .iter()
        .any(|w| matches!(*w, "--tail" | "--max-output"))
}

/// Splits off the `cut` that ends the filter chain. It only shows what the rewritten command
/// prints, so it stays behind it. Returns the other filters and the text to append.
pub(crate) fn split_cut<'a>(filters: &'a [&'a str]) -> (&'a [&'a str], String) {
    match filters.split_last() {
        Some((last, rest)) if filter_name(last).as_deref() == Some("cut") => {
            (rest, format!(" | {last}"))
        }
        _ => (filters, String::new()),
    }
}

/// Returns the refusal for a pipe inside the quoted ctx command, as in
/// `acus ctx 'build | tail -40' | head -60`: the pipe goes and its filter becomes a ctx flag.
pub(crate) fn inner_pipe_reason(inner: &str, filter: &str) -> String {
    let at = unquoted_pipe(inner).unwrap_or(inner.len());
    let build = inner[..at].trim().trim_end_matches("2>&1").trim_end();
    let stage = inner[at..].trim_start_matches('|').trim();
    let (send, how) = if unquoted_pipe(stage).is_some() {
        (
            "acus ctx 'COMMAND'".to_owned(),
            "use acus ctx's own flags, `--tail N` or `--grep RE`".to_owned(),
        )
    } else {
        let flag = (!stage.contains(['<', '>']))
            .then(|| {
                head(stage)
                    .and_then(|(f, args)| ctx_flag(&f.to_lowercase(), &args.collect::<Vec<_>>()))
            })
            .flatten();
        let arg = ctx_arg(build).unwrap_or_else(|| "'COMMAND'".to_owned());
        match flag {
            Some((flag, what)) => (
                format!("acus ctx {arg} {flag}"),
                format!("use the flag `{flag}`, which {what}"),
            ),
            None => (
                format!("acus ctx {arg}"),
                "use acus ctx's own flags, `--tail N` or `--grep RE`".to_owned(),
            ),
        }
    };
    format!(
        "Use `{send}` instead of piping acus ctx into {filter}: drop the pipe `| {stage}` inside the quoted command and {how}. {ESCAPE}\nSend instead: {send}"
    )
}
