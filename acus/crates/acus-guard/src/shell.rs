// SPDX-License-Identifier: MPL-2.0
//! Owns shell text: splitting one command into words, and quoting.

use super::*;

/// Returns `seg` with each bare word that holds backslash-escaped spaces (`Application\ Support`)
/// in single quotes instead, so the rules read it as the one word the shell makes of it. A word
/// with any other escape, an expansion or a wildcard stays as it is.
pub(super) fn quote_escaped_spaces(seg: &str) -> String {
    if !seg.contains("\\ ") {
        return seg.to_owned();
    }
    let (mut out, mut at) = (String::new(), 0);
    for w in split_words(seg) {
        let start = w.as_ptr() as usize - seg.as_ptr() as usize;
        let bare = !w.starts_with(['\'', '"']);
        let rest = w.replace("\\ ", " ");
        if bare
            && w.contains("\\ ")
            && !rest.contains(['\\', '\'', '"', '$', '`', '*', '?', '[', '{', '>', '<'])
        {
            out.push_str(&seg[at..start]);
            out.push_str(&format!("'{rest}'"));
            at = start + w.len();
        }
    }
    out.push_str(&seg[at..]);
    out
}

/// Splits on whitespace into words, with quoted text kept together (quotes included) and escaped
/// characters (`a\ b`, `\'`) kept inside their word.
pub(super) fn split_words(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quote, mut escaped) = (None, None, false);
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (None, '\\') | (Some('"'), '\\') => {
                escaped = true;
                start = start.or(Some(i));
            }
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => {
                quote = Some(c);
                start = start.or(Some(i));
            }
            (None, c) if c.is_whitespace() => out.extend(start.take().map(|st| &s[st..i])),
            _ => start = start.or(Some(i)),
        }
    }
    out.extend(start.map(|st| &s[st..]));
    out
}

/// Returns the command word of a simple command, past assignments and keywords, and its arguments.
pub(super) fn head(seg: &str) -> Option<(&str, impl Iterator<Item = &str>)> {
    let mut words = split_words(seg).into_iter().skip_while(|w| {
        is_assignment(w) || matches!(*w, "do" | "then" | "else" | "{" | "!" | "time" | "sudo")
    });
    let mut head = command_name(words.next()?);
    if matches!(head, "uv" | "env" | "exec" | "npx" | "bunx") {
        // `uv run python …` runs the interpreter.
        head = command_name(
            words
                .by_ref()
                .find(|w| !w.starts_with('-') && *w != "run" && !is_assignment(w))?,
        );
    }
    Some((head, words))
}

/// Returns a command name: the file name without its directory (either separator) and `.exe`.
pub(super) fn command_name(w: &str) -> &str {
    let w = w.trim_matches(['\'', '"']);
    let w = w.rsplit(['/', '\\']).next().unwrap_or(w);
    w.strip_suffix(".exe").unwrap_or(w)
}

/// Returns a word without its outer quotes; a quote of the other kind inside stays.
pub(super) fn unquote(w: &str) -> &str {
    ['\'', '"']
        .iter()
        .find_map(|q| w.strip_prefix(*q)?.strip_suffix(*q))
        .unwrap_or_else(|| w.trim_matches(['\'', '"']))
}

/// Quotes a shell word: single-quoted unless the text holds a single quote.
pub(super) fn quote(s: &str) -> String {
    if s.contains('\'') {
        let escaped: String = s
            .chars()
            .flat_map(|c| {
                let esc = matches!(c, '"' | '\\' | '$' | '`').then_some('\\');
                esc.into_iter().chain([c])
            })
            .collect();
        format!("\"{escaped}\"")
    } else {
        format!("'{s}'")
    }
}

/// Returns `raw` (a path word as the user wrote it) with `suffix` added, as one word for a
/// suggestion. A word the shell expands (a wildcard, or `$VAR` outside single quotes) is kept as
/// written, so the expansion happens as it did; any other word is quoted by its value.
pub(super) fn path_addr(raw: &str, suffix: &str) -> String {
    let expands = has_glob(raw) || (raw.contains('$') && !raw.starts_with('\''));
    if expands {
        format!("{raw}{suffix}")
    } else {
        quote(&format!("{}{suffix}", unquote(raw)))
    }
}

/// Returns a segment run through the `command ` escape, and the segment without it.
pub(super) fn unescape(seg: &str) -> (bool, &str) {
    match seg.trim_start().strip_prefix("command ") {
        Some(rest) => (true, rest),
        None => (false, seg),
    }
}

/// Returns a segment on one line, cut to a length that reads in a refusal.
pub(super) fn one_line(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(160) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

pub(super) fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(k, _)| {
        !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Unquoted wildcards would expand differently once the words are quoted.
pub(super) fn has_glob(seg: &str) -> bool {
    let mut quote = None;
    seg.chars().any(|c| match (quote, c) {
        (Some(q), _) => {
            if c == q {
                quote = None;
            }
            false
        }
        (None, '\'' | '"') => {
            quote = Some(c);
            false
        }
        (None, c) => matches!(c, '*' | '?' | '[' | '{'),
    })
}

/// Reports whether `seg` has a `$NAME`, `${`, `$(` or backtick outside single quotes, which the
/// shell expands before the tool runs. Quoting cannot keep such a word as its text.
pub(super) fn expands_shell(seg: &str) -> bool {
    let mut single = false;
    let mut chars = seg.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => single = !single,
            '\\' if !single => {
                chars.next();
            }
            '`' if !single => return true,
            '$' if !single
                && chars
                    .peek()
                    .is_some_and(|n| n.is_ascii_alphanumeric() || matches!(n, '_' | '{' | '(')) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Expands a leading `~` to the home directory (`scope` knows it), since quoting the path would
/// stop the shell expanding it.
pub(super) fn expand_home(seg: &str, scope: Option<&Scope>) -> Option<String> {
    #[expect(clippy::unwrap_used, reason = "a constant pattern")]
    static TILDE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(^|[\s'"=,:])~(/|$|[\s'"])"#).unwrap());
    if !seg.contains('~') {
        return Some(seg.to_owned());
    }
    let home = home_dir(scope)?;
    if home.is_empty() || home.contains([' ', '\'', '"']) {
        return None;
    }
    let out = TILDE.replace_all(seg, |c: &regex::Captures| {
        format!("{}{home}{}", &c[1], &c[2])
    });
    (!TILDE.is_match(&out) && !out.contains("~")).then(|| out.into_owned())
}

/// Reports whether a shell word's text is what the command receives: single quoted, double quoted
/// without escapes or expansions, or bare without any. `--opt=VALUE` is judged by its value.
pub(super) fn plain_word(w: &str) -> bool {
    plain_text(w)
        && (w.starts_with(['\'', '"']) || !(w.contains(['*', '?', '[', '{']) || w.starts_with('~')))
}

/// Reports whether a word is plain, like `plain_word`, but a bare word may hold wildcards: the
/// caller judges those.
pub(super) fn plain_text(w: &str) -> bool {
    let w = if w.starts_with("--") {
        w.split_once('=').map_or(w, |(_, v)| v)
    } else {
        w
    };
    match w.chars().next() {
        Some('\'') => w.len() >= 2 && w.ends_with('\'') && !w[1..w.len() - 1].contains('\''),
        Some('"') => {
            // Inside double quotes a backslash only escapes `\`, `$`, `` ` `` and `"`.
            let inner = &w[1..w.len().saturating_sub(1).max(1)];
            let mut chars = inner.chars();
            let mut plain = w.len() >= 2 && w.ends_with('"');
            while let Some(c) = chars.next() {
                match c {
                    '"' | '$' | '`' => plain = false,
                    '\\' => {
                        plain &=
                            !matches!(chars.next(), None | Some('\\' | '$' | '`' | '"' | '\n'));
                    }
                    _ => {}
                }
            }
            plain
        }
        _ => !w.contains(['\\', '$', '`', '\'', '"']),
    }
}

/// Reports whether the pipe segment `last` is piped on into any further segment, filter or not.
pub(super) fn continues(segs: &[Segment], last: usize) -> bool {
    segs.get(last + 1).is_some_and(|s| segs[last].feeds(s))
}
