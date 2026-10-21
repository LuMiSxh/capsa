// SPDX-License-Identifier: MPL-2.0

//! Owns the whole-file read checks and the `cat` and `head` reads that fold into acus calls.

use super::*;

mod window;

pub(super) use window::*;

/// Lists the extensions the guard treats as source: every language acus can outline (`Lang::ALL`,
/// so a new language is picked up) plus common languages it cannot, whose reads and edits get the
/// same `acus show` / `acus find` / `acus patch` advice.
pub(super) static CODE: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    const OTHER: &[&str] = &[
        "vue", "go", "cc", "cpp", "hpp", "java", "kt", "rb", "lua", "cs", "sql",
    ];
    Lang::extensions().chain(OTHER.iter().copied()).collect()
});

/// Sets the line count above which a source file is not read whole without a reason.
pub(super) const READ_MAX: usize = 300;

/// Sets how many symbols are listed when such a read is refused.
pub(super) const OUTLINE_MAX: usize = 60;

/// Refuses a whole read of a large source file with the file's outline, so the agent can pick
/// symbols or a range instead.
pub fn read_check(
    input: &serde_json::Value,
    cwd: Option<&str>,
    scope: Option<&Scope>,
) -> Option<Verdict> {
    if ["offset", "limit", "pages"]
        .iter()
        .any(|k| !input[k].is_null())
    {
        return None;
    }
    let file = input["file_path"].as_str()?;
    if !inside(scope, file) {
        return None;
    }
    let path = Path::new(file);
    // Data files are mostly read whole on purpose; their outline says little.
    let lang =
        Lang::from_path(path).filter(|l| !matches!(l, Lang::Toml | Lang::Json | Lang::Yaml))?;
    let src = std::fs::read_to_string(path).ok()?;
    read_verdict(&relative(file, cwd), lang, &src)
}

pub(super) fn read_verdict(path: &str, lang: Lang, src: &str) -> Option<Verdict> {
    let lines = src.lines().count();
    if lines <= READ_MAX {
        return None;
    }
    let o = outline(lang, src)?;
    let mut syms: Vec<String> = o
        .symbols
        .iter()
        .filter(|s| s.depth < 2)
        .map(|s| {
            let name = s.detail.as_deref().unwrap_or(&s.name);
            let indent = "  ".repeat(s.depth);
            format!("{indent}{} {name} {}-{}", s.kind, s.start_line, s.end_line)
        })
        .collect();
    if syms.len() > OUTLINE_MAX {
        let more = syms.len() - OUTLINE_MAX;
        syms.truncate(OUTLINE_MAX);
        syms.push(format!("… {more} more (acus outline {path})"));
    }
    Some(Verdict::Deny(
        Rule::ReadTool,
        format!(
            "{path} has {lines} lines. Read only what you need: `acus show {path}#Symbol {path}:A-B` reads several symbols and ranges in one call, or Read with offset and limit. Only if you need the whole file, Read it with limit: {lines}.\nOutline of {path}:\n{}",
            syms.join("\n")
        ),
    ))
}

/// Returns the `head -n N FILE` that `cat FILE | head [-n] N` reads, when both segments are single
/// words and plain, so nothing else is cut or changed.
pub(super) fn cat_head(seg: &str, next: &str) -> Option<String> {
    let (seg, tail) = stderr_tail(seg.trim());
    let cat = split_words(seg);
    let [c, file] = cat.as_slice() else {
        return None;
    };
    if *c != "cat" || file.starts_with('-') {
        return None;
    }
    let h = split_words(next.trim());
    // Digits only: `parse` also takes a `+`, and `tail -n +N` reads from line N, not N lines.
    let count = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| s.parse::<usize>().ok())
            .flatten()
            .filter(|n| *n >= 1)
    };
    // `tail -n +N`, `tail +N` and `tail --lines=+N` read from line N to the end, not N lines.
    if let ["tail", opts @ ..] = h.as_slice()
        && let Some(n) = from_line(opts)
    {
        return Some(format!("tail -n +{n} {file}{tail}"));
    }
    let (cmd, n) = match h.as_slice() {
        [f @ ("head" | "tail")] => (*f, 10),
        [f @ ("head" | "tail"), "-n", n] => (*f, count(n)?),
        [f @ ("head" | "tail"), a] => (
            *f,
            count(a.strip_prefix("-n").or_else(|| a.strip_prefix('-'))?)?,
        ),
        _ => return None,
    };
    Some(format!("{cmd} -n {n} {file}{tail}"))
}

/// Splits off the stderr redirect that ends a `cat` or `head` read, which the read keeps:
/// `2>/dev/null` (also `2> /dev/null`, written the first way) or `2>&1`. Returns the text before
/// it and the redirect.
pub(super) fn stderr_tail(seg: &str) -> (&str, &'static str) {
    [
        ("2>/dev/null", " 2>/dev/null"),
        ("2> /dev/null", " 2>/dev/null"),
        ("2>&1", " 2>&1"),
    ]
    .into_iter()
    .find_map(|(redirect, kept)| {
        let rest = seg.strip_suffix(redirect)?;
        rest.ends_with(char::is_whitespace)
            .then(|| (rest.trim_end(), kept))
    })
    .unwrap_or((seg, ""))
}

pub(super) fn rewrite_read(rule: Rule, seg: &str, scope: Option<&Scope>) -> Option<String> {
    // A trailing `2>/dev/null` or `2>&1` is kept as it is.
    let t = seg.trim_end();
    let (seg, tail) = ["2>/dev/null", "2>&1"]
        .iter()
        .find_map(|s| {
            let r = t.strip_suffix(s)?.trim_end();
            (r.len() < t.len() - s.len()).then(|| (r, &t[r.len()..]))
        })
        .unwrap_or((seg, ""));
    if let Some(read) = sed_to_end(seg, scope) {
        return Some(format!("{read}{tail}"));
    }
    // Only a grep pattern may hold these as text: `<` and `>` inside any quotes, `$` in single.
    if seg.contains(['<', '>', '$']) && !(rule == Rule::Grep && literal_specials(seg)) {
        return None;
    }
    let seg = expand_home(seg, scope)?;
    let words = split_words(&seg);
    // Word by word, so that the unquoted `--include=*.rs` passes as grep itself sees it.
    let name_filter = |w: &str| {
        ["--include=", "--exclude=", "--exclude-dir="]
            .iter()
            .any(|p| w.starts_with(p))
    };
    // An unquoted glob or brace word is expanded by the shell for the rewrite too, but only where
    // the expanded words mean the same: the operands of grep, the files of cat.
    let glob_ok = matches!(
        words.first().copied(),
        Some("cat" | "grep" | "egrep" | "fgrep")
    );
    if words
        .iter()
        .any(|w| !plain_text(w) || (!name_filter(w) && !plain_word(w) && !(glob_ok && has_glob(w))))
    {
        return None;
    }
    let (&first, args) = words.split_first()?;
    if !matches!(
        first,
        "cat" | "head" | "tail" | "sed" | "grep" | "egrep" | "fgrep" | "rg" | "find"
    ) {
        return None;
    }
    // File listings become `acus files`; only `find -name` and `rg --files` have one.
    if first == "find" {
        return (rule == Rule::GlobTool)
            .then(|| find_files(args))
            .flatten()
            .map(|s| format!("{s}{tail}"));
    }
    if first == "rg" && args.contains(&"--files") {
        return rg_files(args).map(|s| format!("{s}{tail}"));
    }
    // A file with an extension keeps the code check; an extension-less text file must exist.
    let file = |a: &str| file_operand(a, scope);
    let ok = match (rule, first) {
        (Rule::Grep, _) => !names_non_file(first, args, scope),
        // `cat -n` numbers the lines of several files in one run; `acus show` restarts per file.
        (Rule::ReadShell, "cat") => match args {
            ["-n" | "--number", f] => file(f) && !has_glob(f),
            _ => {
                // `acus show` exits 0 with one of several files missing, unlike cat: each literal
                // file must exist (a glob is expanded by the shell and is judged by its words).
                let several = args.len() > 1;
                !args.is_empty()
                    && args.iter().all(|a| {
                        if has_glob(a) {
                            brace_words(a).iter().all(|f| file(f.as_str()))
                        } else {
                            file(a) && (!several || !is_dir(scope, a))
                        }
                    })
            }
        },
        // `head FILE`, `head -n N FILE`, `head -N FILE`; `tail` likewise (`tail -f`, `-c` and
        // `+N` are other reads).
        (Rule::ReadShell, "head") => line_count_args(args).is_some_and(|(_, f)| file(f)),
        (Rule::ReadShell, "tail") => tail_from(args)
            .map(|(_, f)| f)
            .or_else(|| line_count_args(args).map(|(_, f)| f))
            .is_some_and(file),
        (Rule::ReadShell, "sed") => match args {
            ["-n", script, f] => file(f) && sed_lines(unquote(script)).is_some(),
            _ => false,
        },
        _ => false,
    };
    // `show_cmd` takes one file per word and no comma: brace groups are kept, as expanded.
    if ok && first == "cat" && args.iter().any(|a| a.contains(',')) {
        return Some(format!("acus show {}{tail}", args.join(" ")));
    }
    ok.then(|| suggest(rule, &seg))
        .flatten()
        .map(|s| format!("{}{tail}", s.trim_matches('`')))
}
