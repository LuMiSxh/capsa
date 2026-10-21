// SPDX-License-Identifier: MPL-2.0

//! Owns the line windows of `head`, `tail` and `sed -n` reads, and the files they may read.

use super::*;

/// Names the rows a `head` or `tail` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Window {
    First(usize),
    Last(usize),
    /// From this line to the end (`tail -n +N`).
    From(usize),
}

/// Returns the window of `head` or `tail` options (`cmd` is "head" or "tail"; `opts` are the words
/// after it, without the file): none (ten lines), `-n N`, `-nN`, `-N`, `--lines=N` and
/// `--lines N`, and for `tail` also the `+N` of each. A count is at least 1 and plain digits, so
/// `-c` is no count.
pub(crate) fn window(cmd: &str, opts: &[&str]) -> Option<Window> {
    window_min(cmd, opts, 1)
}

/// Returns the smallest count `window` accepts. A read needs one line at least; a build filter
/// (`swift build | head -n 0`) folds to ctx with a count of zero too.
pub(crate) fn window_min(cmd: &str, opts: &[&str], min: usize) -> Option<Window> {
    let tail = match cmd {
        "head" => false,
        "tail" => true,
        _ => return None,
    };
    let count = |s: &str| digits(s).filter(|n| *n >= min);
    // `-N` has no `+` form, but the value of `-n` and `--lines` has one for `tail`.
    let plain = |s: &str| {
        count(s).map(|n| {
            if tail {
                Window::Last(n)
            } else {
                Window::First(n)
            }
        })
    };
    let valued = |s: &str| match s.strip_prefix('+') {
        Some(n) if tail => count(n).map(Window::From),
        _ => plain(s),
    };
    match opts {
        [] => Some(if tail {
            Window::Last(10)
        } else {
            Window::First(10)
        }),
        [o] => {
            let o = unquote(o);
            if let Some(v) = o.strip_prefix("--lines=").or_else(|| o.strip_prefix("-n")) {
                valued(v)
            } else if let Some(v) = o.strip_prefix('-') {
                plain(v)
            } else if tail && o.starts_with('+') {
                valued(o)
            } else {
                None
            }
        }
        [o, v] if matches!(unquote(o), "-n" | "--lines") => valued(unquote(v)),
        _ => None,
    }
}

/// Returns the line count and the file of `head`/`tail` arguments: the `head` forms of `window`,
/// then the file.
pub(crate) fn line_count_args<'a>(args: &[&'a str]) -> Option<(usize, &'a str)> {
    let (&file, opts) = args.split_last()?;
    match window("head", opts)? {
        Window::First(n) => Some((n, file)),
        _ => None,
    }
}

/// Returns the line ranges of a `sed -n` script of plain line addresses: `3p`, `7,9p`, or several
/// joined by `;` (`3p;7,9p`). Returns `None` for anything else, such as a regex address or a
/// command other than `p`.
pub(crate) fn sed_lines(script: &str) -> Option<Vec<(usize, usize)>> {
    script
        .split(';')
        .map(|part| {
            let r = part.strip_suffix('p')?;
            let (a, b) = r.split_once(',').unwrap_or((r, r));
            let a: usize = a.parse().ok()?;
            let b: usize = b.parse().ok()?;
            (a >= 1 && a <= b).then_some((a, b))
        })
        .collect()
}

/// Returns the N of `tail -n +N`, `tail -n+N`, `tail +N` or `tail --lines=+N` options: the read
/// from line N to the end. `+0`, and a count with no `+`, are other reads.
pub(crate) fn from_line(opts: &[&str]) -> Option<usize> {
    match window("tail", opts)? {
        Window::From(n) => Some(n),
        _ => None,
    }
}

/// Parses plain digits as a number: a sign, a `+` or empty text is not.
pub(crate) fn digits(s: &str) -> Option<usize> {
    s.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| s.parse().ok())
        .flatten()
}

/// Returns the line N and file of `tail -n +N FILE`, `tail +N FILE` or `tail --lines=+N FILE`.
pub(crate) fn tail_from<'a>(args: &[&'a str]) -> Option<(usize, &'a str)> {
    let (&file, opts) = args.split_last()?;
    if file.starts_with('+') {
        return None;
    }
    Some((from_line(opts)?, file))
}

/// Returns the line count of `head` options: none (ten lines), `-n N`, `-N` or `-nN`.
pub(crate) fn head_count(opts: &[&str]) -> Option<usize> {
    match window("head", opts)? {
        Window::First(n) => Some(n),
        _ => None,
    }
}

/// Returns the N of a `sed -n 'N,$p'` script, or `"N,\$p"` in double quotes: the read from line N
/// to the end. A bare word is refused, since its `$` would be the shell's.
pub(crate) fn sed_from(word: &str) -> Option<usize> {
    let body = match word.as_bytes() {
        [b'\'', .., b'\''] => word[1..word.len() - 1].to_owned(),
        [b'"', .., b'"'] => word[1..word.len() - 1].replace("\\$", "$"),
        _ if word.contains('$') => return None,
        _ => word.to_owned(),
    };
    digits(body.strip_suffix(",$p")?).filter(|n| *n >= 1)
}

/// Reports whether an operand is a file that `acus show` can name: no comma or flag, and either a
/// code path or an extension-less file that exists.
/// Reports whether every `<`, `>` and `$` of a segment is quoted text: the shell reads `<` and `>`
/// as text in any quotes, and `$` only in single quotes.
pub(crate) fn literal_specials(seg: &str) -> bool {
    let mut quote = None;
    seg.chars().all(|c| match (quote, c) {
        (Some('\''), '\'') | (Some('"'), '"') => {
            quote = None;
            true
        }
        (Some('\''), _) => true,
        (Some(_), '$') => false,
        (Some(_), _) => true,
        (None, '\'' | '"') => {
            quote = Some(c);
            true
        }
        (None, c) => !matches!(c, '<' | '>' | '$'),
    })
}

/// Returns the number of lines of a file under the scope: its newlines, and a last line without
/// one. Returns `None` where the file cannot be read.
pub(crate) fn file_lines(scope: Option<&Scope>, path: &str) -> Option<usize> {
    use std::io::Read;
    let path = match scope {
        Some(s) => s.path(path)?,
        None => PathBuf::from(unquote(path)),
    };
    let mut file = std::fs::File::open(path).ok()?;
    let (mut lines, mut last) = (0, b'\n');
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        lines += buf[..n].iter().filter(|&&b| b == b'\n').count();
        last = buf[n - 1];
    }
    Some(lines + usize::from(last != b'\n'))
}

pub(crate) fn file_operand(a: &str, scope: Option<&Scope>) -> bool {
    let p = unquote(a);
    !a.contains(',')
        && !a.starts_with('-')
        && if Path::new(&p).extension().is_some() {
            is_code_path(p)
        } else {
            is_file(scope, p)
        }
}

/// Returns `sed -n 'N,$p' FILE` or `sed -n "N,\$p" FILE` as the `acus show` call of that read. The
/// `$` is literal in the script, so this runs before the `$` refusal in `rewrite_read`.
pub(crate) fn sed_to_end(seg: &str, scope: Option<&Scope>) -> Option<String> {
    let seg = expand_home(seg, scope)?;
    let words = split_words(&seg);
    let [sed, flag, script, file] = words.as_slice() else {
        return None;
    };
    if *sed != "sed" || *flag != "-n" || sed_from(script).is_none() {
        return None;
    }
    if file.contains(['$', '<', '>'])
        || !plain_word(file)
        || has_glob(file)
        || !file_operand(file, scope)
    {
        return None;
    }
    suggest(Rule::ReadShell, &seg).map(|s| s.trim_matches('`').to_string())
}

/// Reports whether a word names a file for `acus show`: not a flag, a redirect or a comma list.
pub(crate) fn operand(w: &str) -> bool {
    !w.starts_with('-') && !w.contains(['<', '>', ','])
}

/// Reports whether an operand is clearly not source: a file with an extension that is neither code
/// nor has wildcards (logs, text, data formats).
pub(crate) fn is_plain_file(w: &str) -> bool {
    let w = unquote(w);
    let name = w.rsplit(['/', '\\']).next().unwrap_or(w);
    name.rsplit_once('.').is_some_and(|(_, e)| {
        !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()) && !is_source_path(name)
    })
}

pub(crate) fn is_code_path(w: &str) -> bool {
    !w.starts_with('-')
        && w.rsplit_once('.')
            .is_some_and(|(_, e)| CODE.contains(&e.to_ascii_lowercase().as_str()))
}

/// Reports whether a file is source rather than data: `.json`, `.toml` and `.yaml` files are
/// config and data, which agents read whole on purpose (the Read tool leaves them alone, too).
pub(crate) fn is_source_path(w: &str) -> bool {
    is_code_path(w)
        && !w.rsplit_once('.').is_some_and(|(_, e)| {
            ["json", "toml", "yaml", "yml"].contains(&e.to_ascii_lowercase().as_str())
        })
}

/// Returns the `acus show` or `acus find` call that does what a `cat`, `head -n N`, `sed -n A,Bp`
/// or `grep` of the segment did, or `None` where the replacement would not be the same read.
/// Returns the words a brace group with a comma expands to (`src/{a,b}.rs`), or the word itself.
pub(crate) fn brace_words(w: &str) -> Vec<String> {
    match (w.find('{'), w.find('}')) {
        (Some(open), Some(close))
            if open < close
                && w[open + 1..close].contains(',')
                && !w[open + 1..close].contains('{') =>
        {
            w[open + 1..close]
                .split(',')
                .map(|alt| format!("{}{alt}{}", &w[..open], &w[close + 1..]))
                .collect()
        }
        _ => vec![w.to_owned()],
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use Window::*;

    #[test]
    fn head_and_tail_forms() {
        for (c, o, w) in [
            ("head", vec![], Some(First(10))),
            ("head", vec!["-n", "5"], Some(First(5))),
            ("head", vec!["-n5"], Some(First(5))),
            ("head", vec!["-5"], Some(First(5))),
            ("head", vec!["--lines=5"], Some(First(5))),
            ("head", vec!["--lines", "5"], Some(First(5))),
            ("tail", vec!["-3"], Some(Last(3))),
            ("tail", vec!["-n", "+4"], Some(From(4))),
            ("tail", vec!["-n+4"], Some(From(4))),
            ("tail", vec!["+4"], Some(From(4))),
            ("tail", vec!["--lines=+4"], Some(From(4))),
            ("tail", vec!["--lines", "+4"], Some(From(4))),
            ("head", vec!["-n", "0"], None),
            ("tail", vec!["-f"], None),
            ("head", vec!["-c", "5"], None),
            ("head", vec!["-n", "'7'"], Some(First(7))),
        ] {
            assert_eq!(window(c, &o), w, "{c} {o:?}");
        }
    }
}
