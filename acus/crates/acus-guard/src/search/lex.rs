// SPDX-License-Identifier: MPL-2.0

//! Lexes the options of a grep or rg command line into the words `acus find` takes.

use super::*;

/// Lists the long options that take a value in the next word when none is attached with `=`.
pub(crate) const LONG_WITH_VALUE: &[&str] = &[
    "regexp",
    "file",
    "glob",
    "type",
    "max-count",
    "include",
    "exclude",
    "exclude-dir",
    "after-context",
    "before-context",
    "context",
    "max-depth",
];

/// Lists the short options that take a value: the rest of their cluster, or else the next word.
pub(crate) const SHORT_WITH_VALUE: &str = "efgtmABCdDTjM";

/// Splits grep or rg arguments into options and words.
pub(crate) fn lex_grep<'a>(args: &[&'a str]) -> Vec<GrepOpt<'a>> {
    let mut out = Vec::new();
    let mut words = args.iter().copied();
    while let Some(raw) = words.next() {
        if raw == "--" {
            out.extend(words.by_ref().map(GrepOpt::Word));
            break;
        }
        lex_word(raw, &mut words, &mut out);
    }
    out
}

/// Lexes `raw` (not `--`) and takes the words that its options use as values from `words`.
pub(crate) fn lex_word<'a>(
    raw: &'a str,
    words: &mut impl Iterator<Item = &'a str>,
    out: &mut Vec<GrepOpt<'a>>,
) {
    if raw == "-" {
        out.push(GrepOpt::Stdin);
    } else if let Some(long) = raw.strip_prefix("--") {
        let (name, attached) = long
            .split_once('=')
            .map_or((long, None), |(n, v)| (n, Some(v)));
        let value = attached.or_else(|| {
            LONG_WITH_VALUE
                .contains(&name)
                .then(|| words.next())
                .flatten()
        });
        out.push(GrepOpt::Long(name, value));
    } else if let Some(cluster) = raw.strip_prefix('-').filter(|c| !c.is_empty()) {
        if cluster.bytes().all(|b| b.is_ascii_digit()) {
            out.push(GrepOpt::Context(cluster));
            return;
        }
        for (i, c) in cluster.char_indices() {
            if SHORT_WITH_VALUE.contains(c) {
                let rest = &cluster[i + c.len_utf8()..];
                let value = if rest.is_empty() {
                    words.next()
                } else {
                    Some(rest)
                };
                out.push(GrepOpt::Short(c, value));
                break;
            }
            out.push(GrepOpt::Short(c, None));
        }
    } else {
        out.push(GrepOpt::Word(raw));
    }
}

/// Sets `-g` or `-t` with its value; a missing value has no equivalent.
pub(crate) fn push_value(s: &mut Search<'_>, flag: &str, v: Option<&str>) {
    match v {
        Some(v) => s.flags.push(format!("{flag} {}", quote(unquote(v)))),
        None => s.inexact = true,
    }
}

/// Parses `-A`, `-B` or `-C` with its line count.
pub(crate) fn push_context(s: &mut Search<'_>, c: char, v: Option<&str>) {
    match v.and_then(|v| v.parse::<usize>().ok()) {
        Some(n) => s.flags.push(format!("-{c} {n}")),
        None => s.inexact = true,
    }
}

/// Handles `--include`, `--exclude` and `--exclude-dir`, which match names, not paths: a glob with
/// a `/` is something else for `acus find`. Braces are literal for grep (`*.{rs,md}` matches
/// nothing), so a glob with them is not translated.
pub(crate) fn push_name_glob(s: &mut Search<'_>, v: Option<&str>, exclude: bool) {
    if let Some(items) = v.and_then(plain_brace_list) {
        for item in items {
            push_name_glob(s, Some(item), exclude);
        }
        return;
    }
    match v
        .map(unquote)
        .filter(|g| !g.is_empty() && !g.contains(['/', '{', '}']))
    {
        Some(g) => s.flags.push(format!(
            "-g {}",
            quote(&format!("{}{g}", if exclude { "!" } else { "" }))
        )),
        None => s.inexact = true,
    }
}

/// Returns the items of an unquoted `{a,b}` word whose items are all plain names. The shell
/// expands that form into one option per item, so each item is what grep receives. A quote or
/// backslash cannot occur in a plain name, which leaves the quoted forms (they keep their braces
/// for grep) refused.
pub(crate) fn plain_brace_list(v: &str) -> Option<Vec<&str>> {
    let items: Vec<&str> = v.strip_prefix('{')?.strip_suffix('}')?.split(',').collect();
    let plain = |i: &&str| {
        !i.is_empty()
            && i.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    (items.len() > 1 && items.iter().all(plain)).then_some(items)
}

/// Reports whether a glob or brace word ends in a file name with an extension (`src/*.rs`,
/// `{a,b}.md`).
pub(crate) fn names_file(w: &str) -> bool {
    let name = w.rsplit('/').next().unwrap_or(w);
    name.rsplit_once('.')
        .is_some_and(|(_, e)| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Reports whether a grep without `-r` has an operand that is not a file (a directory, most
/// often): grep prints "Is a directory" and searches nothing there, where `acus find` recurses.
/// Globs are left to `parse_search`, which judges them by `names_file`.
pub(crate) fn names_non_file(head: &str, args: &[&str], scope: Option<&Scope>) -> bool {
    let s = parse_search(head, args);
    !s.recursive && s.operands.iter().any(|o| !has_glob(o) && is_dir(scope, o))
}

pub(crate) fn own(v: Option<&str>) -> Option<String> {
    v.map(|p| unquote(p).to_owned())
}

/// Reads `-m N` as the same per-file limit; a count of zero or anything but a number is not.
pub(crate) fn push_max_count(s: &mut Search<'_>, v: Option<&str>) {
    match v.and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0) {
        Some(n) => s.flags.push(format!("-m {n}")),
        None => s.inexact = true,
    }
}

/// Lists the boolean flags that map straight onto an `acus find` flag: `(short, long, acus_flag)`.
pub(crate) const BOOL_FLAGS: &[(Option<char>, &str, &str)] = &[
    (Some('i'), "ignore-case", "-i"),
    (Some('w'), "word-regexp", "-w"),
    (Some('l'), "files-with-matches", "-l"),
    (Some('F'), "fixed-strings", "-F"),
    (Some('c'), "count", "-c"),
    (Some('o'), "only-matching", "-o"),
    (Some('v'), "invert-match", "-v"),
    (None, "hidden", "--hidden"),
    (None, "no-ignore", "-u"),
];
