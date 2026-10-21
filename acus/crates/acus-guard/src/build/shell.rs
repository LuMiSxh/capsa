// SPDX-License-Identifier: MPL-2.0

//! Scans the shell syntax around a build: redirects, pipes and `timeout`.

use super::*;

/// Reports whether `text` has a `<` or `>` outside quotes, which is a redirect. In quotes it is
/// part of a pattern.
pub(crate) fn has_redirect(text: &str) -> bool {
    let (mut quote, mut escaped) = (None, false);
    for c in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (None, '\\') | (Some('"'), '\\') => escaped = true,
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '<' | '>') => return true,
            _ => {}
        }
    }
    false
}

/// Splits a leading `timeout N` or `gtimeout N` (N whole seconds, or with an `s`, `m` or `h`
/// suffix) into the limit in seconds and the command after it. Options of timeout give `None`.
pub(crate) fn strip_timeout(text: &str) -> Option<(u64, &str)> {
    let words = split_words(text);
    let [name, n, ..] = words.as_slice() else {
        return None;
    };
    if !matches!(*name, "timeout" | "gtimeout") {
        return None;
    }
    let (num, mult) = match n.chars().last()? {
        's' => (&n[..n.len() - 1], 1),
        'm' => (&n[..n.len() - 1], 60),
        'h' => (&n[..n.len() - 1], 3600),
        _ => (*n, 1),
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let secs = num
        .parse::<u64>()
        .ok()?
        .checked_mul(mult)
        .filter(|s| *s > 0)?;
    let end = n.as_ptr() as usize - text.as_ptr() as usize + n.len();
    Some((secs, text[end..].trim_start()))
}

/// Returns the byte index of the first `|` outside quotes, if any.
pub(crate) fn unquoted_pipe(text: &str) -> Option<usize> {
    let (mut quote, mut escaped) = (None, false);
    for (i, c) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (None, '\\') | (Some('"'), '\\') => escaped = true,
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '|') => return Some(i),
            _ => {}
        }
    }
    None
}
