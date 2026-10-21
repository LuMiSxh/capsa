// SPDX-License-Identifier: MPL-2.0

//! Parses the search commands the guard knows, and refuses the ones acus find cannot express.

use super::*;

/// Reads grep, rg, `Select-String` and `findstr` arguments into the `acus find` flags that do
/// the same. A flag, pattern source or output mode with no exact equivalent sets `inexact`.
pub(crate) fn parse_search<'a>(head: &str, args: &[&'a str]) -> Search<'a> {
    let ps = matches!(head, "select-string" | "sls");
    let rg = matches!(head, "rg" | "ag" | "ack");
    let mut p = Parsed {
        s: Search {
            recursive: rg,
            ..Search::default()
        },
        have_pattern: false,
        ere: head == "egrep",
        grep: matches!(head, "grep" | "egrep" | "fgrep"),
        rg: head == "rg",
        after_dashes: false,
    };
    // Unquoted globs and braces are expanded by the shell before the tool runs. Counting them as
    // the words are read finds a glob that takes a flag value, which has no exact equivalent.
    let globs = std::cell::Cell::new(0usize);
    let mut words = args
        .iter()
        .copied()
        .inspect(|w| globs.set(globs.get() + usize::from(has_glob(w))));
    while let Some(raw) = words.next() {
        let seen = globs.get();
        let read = if skip_redirection(&mut p.s, raw, &mut words) {
            true
        } else if ps && raw.starts_with('-') {
            parse_select_string(&mut p, &raw.to_lowercase(), &mut words);
            true
        } else if head == "findstr" && is_findstr_switch(raw) {
            parse_findstr(&mut p, raw);
            true
        } else {
            parse_posix_rg(&mut p, raw, rg, &mut words)
        };
        if globs.get() > seen {
            p.s.inexact = true;
        }
        if read {
            continue;
        }
        if p.have_pattern {
            p.s.operands.push(raw);
        } else {
            // The shell expands a glob pattern, so the tool never sees the pattern as written.
            p.s.inexact |= has_glob(raw);
            p.have_pattern = true;
            p.s.pattern = own(Some(raw));
        }
    }
    let Parsed { mut s, ere, .. } = p;
    // A glob or brace operand expands to files, which `acus find` searches as grep does. A
    // non-recursive grep reports a directory such a word may expand to, so only a name with a file
    // extension (`src/*.rs`) is kept.
    if !s.recursive && s.operands.iter().any(|o| has_glob(o) && !names_file(o)) {
        s.inexact = true;
    }
    if head == "fgrep" {
        s.flags.push("-F".into());
    }
    // `grep -r` also searches ignored and hidden files (`.git`, `target`, `node_modules`).
    // `acus find` skips them and says so in its output, which is what the agent wants.
    // Basic and extended syntax become Rust regex, or the search is not converted.
    if matches!(head, "grep" | "egrep" | "fgrep")
        && !s.flags.iter().any(|f| f == "-F")
        && let Some(p) = s.pattern.take()
    {
        match convert::posix_to_rust(&p, ere) {
            Some(r) => s.pattern = Some(r),
            None => {
                s.inexact = true;
                s.pattern = Some(p);
            }
        }
    }
    // Select-String ignores case unless told otherwise.
    if ps
        && !args
            .iter()
            .any(|a| a.eq_ignore_ascii_case("-casesensitive"))
    {
        s.flags.push("-i".into());
    }
    s.flags.dedup();
    mark_unsupported_combinations(&mut s);
    s
}

/// Reports whether a word is a redirection, which is not an operand: `< file` reads the file,
/// `<<<` a string.
pub(crate) fn skip_redirection<'a>(
    s: &mut Search<'a>,
    raw: &'a str,
    words: &mut impl Iterator<Item = &'a str>,
) -> bool {
    let r = raw.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
    if r.starts_with('>') {
        if matches!(r, ">" | ">>") {
            words.next();
        }
        return true;
    }
    let Some(f) = r.strip_prefix('<') else {
        return false;
    };
    match f {
        "<<" => {
            words.next();
        }
        "" | "<" => {}
        f => s.operands.push(f),
    }
    true
}

/// Parses a `Select-String` parameter; `lc` is the lowercased word.
pub(super) fn parse_select_string<'a>(
    p: &mut Parsed<'a>,
    lc: &str,
    words: &mut impl Iterator<Item = &'a str>,
) {
    match lc {
        "-pattern" => p.set_pattern(own(words.next())),
        "-path" | "-literalpath" => p.s.operands.extend(
            words
                .next()
                .into_iter()
                .flat_map(|p| p.split(','))
                .map(unquote),
        ),
        "-casesensitive" => {}
        "-simplematch" => p.s.flags.push("-F".into()),
        "-list" => p.s.flags.push("-l".into()),
        "-notmatch" => p.s.flags.push("-v".into()),
        "-include" | "-exclude" | "-context" | "-encoding" | "-inputobject" => {
            p.s.inexact = true;
            words.next();
        }
        _ => p.s.inexact = true,
    }
}

/// Reads a `findstr` switch: `/S`, `/I`, `/C:text`. A path with a backslash is not one.
pub(crate) fn is_findstr_switch(raw: &str) -> bool {
    raw.starts_with('/')
        && !raw.contains('\\')
        && (raw.len() == 2 || raw.get(2..).is_some_and(|r| r.starts_with(':')))
}

pub(super) fn parse_findstr(p: &mut Parsed<'_>, raw: &str) {
    match raw.to_lowercase().as_bytes()[1] {
        b's' => p.s.recursive = true,
        b'i' => p.s.flags.push("-i".into()),
        b'm' => p.s.flags.push("-l".into()),
        b'v' => p.s.flags.push("-v".into()),
        b'n' => {}
        b'c' => {
            p.set_pattern(own(raw.get(3..)).filter(|p| !p.is_empty()));
            p.s.flags.push("-F".into());
        }
        // `/g:FILE` reads the patterns from a file.
        b'g' => p.patterns_from_file(),
        _ => p.s.inexact = true,
    }
}

/// Reports whether a word is a grep or rg option, long or a cluster of short ones. False for a
/// plain word.
pub(super) fn parse_posix_rg<'a>(
    p: &mut Parsed<'a>,
    raw: &'a str,
    rg: bool,
    words: &mut impl Iterator<Item = &'a str>,
) -> bool {
    // The shell strips the quotes of a wholly quoted option word, so grep reads `'-i'` as `-i`.
    // Quoting keeps braces, globs, `$` and quotes literal, which `acus find` cannot reproduce, so
    // such a word is refused rather than read as a plain option.
    let quoted = raw.starts_with(['\'', '"']) && unquote(raw).starts_with('-');
    let raw = if quoted { unquote(raw) } else { raw };
    if quoted && raw.contains(['{', '}', '[', ']', '*', '?', '$', '`', '\\', '\'', '"']) {
        p.s.inexact = true;
        return true;
    }
    if raw == "--" {
        p.s.inexact = true;
        p.after_dashes = true;
        return true;
    }
    if p.after_dashes {
        return false;
    }
    let mut opts = Vec::new();
    lex_word(raw, words, &mut opts);
    if let [GrepOpt::Word(_)] = opts[..] {
        return false;
    }
    for opt in opts {
        match opt {
            // Standard input is not a file `acus find` can take.
            GrepOpt::Stdin | GrepOpt::Word(_) => p.s.inexact = true,
            // `-3` is `-C 3`.
            GrepOpt::Context(n) => push_context(&mut p.s, 'C', Some(n)),
            GrepOpt::Long(name, value) => match name {
                "recursive" => p.s.recursive = true,
                // Follows every symlink, as `acus find -L`.
                "dereference-recursive" => {
                    p.s.recursive = true;
                    p.flag_if("-L", p.grep);
                }
                "line-regexp" => p.flag_if("-x", p.grep || p.rg),
                "follow" => p.flag_if("-L", p.rg),
                "multiline" => p.flag_if("-U", p.rg),
                "line-number" => {}
                "extended-regexp" => p.ere = true,
                "regexp" => p.set_pattern(own(value)),
                "file" => p.patterns_from_file(),
                "glob" => push_value(&mut p.s, "-g", value),
                "type" => push_value(&mut p.s, "-t", value),
                "max-count" => push_max_count(&mut p.s, value),
                "include" => push_name_glob(&mut p.s, value, false),
                "exclude" | "exclude-dir" => push_name_glob(&mut p.s, value, true),
                "after-context" => push_context(&mut p.s, 'A', value),
                "before-context" => push_context(&mut p.s, 'B', value),
                "context" => push_context(&mut p.s, 'C', value),
                "max-depth" => p.s.inexact = true,
                _ => match BOOL_FLAGS.iter().find(|f| f.1 == name) {
                    Some(f) => p.s.flags.push(f.2.into()),
                    None => p.s.inexact = true,
                },
            },
            GrepOpt::Short(c, value) => match c {
                'r' if !rg => p.s.recursive = true,
                'R' if !rg => {
                    p.s.recursive = true;
                    p.flag_if("-L", p.grep);
                }
                // grep's `-L` lists files without a match and its `-U` reads binary files.
                'x' => p.flag_if("-x", p.grep || p.rg),
                'L' => p.flag_if("-L", p.rg),
                'U' => p.flag_if("-U", p.rg),
                'E' => p.ere = true,
                'n' => {}
                'e' => p.set_pattern(own(value)),
                'f' => p.patterns_from_file(),
                'g' => push_value(&mut p.s, "-g", value),
                't' => push_value(&mut p.s, "-t", value),
                'm' => push_max_count(&mut p.s, value),
                'A' | 'B' | 'C' => push_context(&mut p.s, c, value),
                // Flags with a value that `acus find` does not take.
                'd' | 'D' | 'T' | 'j' | 'M' => p.s.inexact = true,
                _ => match BOOL_FLAGS.iter().find(|f| f.0 == Some(c)) {
                    Some(f) => p.s.flags.push(f.2.into()),
                    None => p.s.inexact = true,
                },
            },
        }
    }
    true
}

/// `acus find` shows one output mode at a time, and `-m` with `-o` or `-v` counts differently.
/// It prints context lines for hits only, not with counts, file lists or matched text, and not
/// together with `-m`. It also needs a pattern.
pub(crate) fn mark_unsupported_combinations(s: &mut Search<'_>) {
    let modes = s
        .flags
        .iter()
        .filter(|f| matches!(f.as_str(), "-c" | "-l" | "-o" | "-v"))
        .count();
    let limited = s.flags.iter().any(|f| f.starts_with("-m "));
    let limited_mode = s.flags.iter().any(|f| matches!(f.as_str(), "-o" | "-v"));
    let context = s.flags.iter().any(|f| {
        f.starts_with(['-'])
            && f.as_bytes().get(2) == Some(&b' ')
            && matches!(f.as_bytes()[1], b'A' | b'B' | b'C')
    });
    let context_off = limited
        || s.flags
            .iter()
            .any(|f| matches!(f.as_str(), "-c" | "-l" | "-o"));
    // `-U` does not count, invert or print context; `-x` replaces `-w`.
    let has = |flag: &str| s.flags.iter().any(|f| f == flag);
    let multiline_clash = has("-U") && (context || has("-c") || has("-v"));
    let line_word = has("-x") && has("-w");
    if (context && context_off)
        || modes > 1
        || (limited && limited_mode)
        || s.pattern.is_none()
        || multiline_clash
        || line_word
    {
        s.inexact = true;
    }
}
