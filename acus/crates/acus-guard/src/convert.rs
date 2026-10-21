// SPDX-License-Identifier: MPL-2.0
//! Owns the translation of grep patterns for the guard's rewrites: what `grep` reads as a POSIX
//! basic or extended regular expression becomes the Rust regex that `acus find` and
//! `acus ctx --grep` take. Only the constructs that mean the same in both are translated; anything
//! else gives `None`, so the command is not rewritten instead of searching for something else.

/// Translates `pat` into a Rust regex: grep's basic syntax (`ere` false: `\|`, `\(`, `\{` and `\+`
/// operate, bare `| + ? ( ) { }` are literal) or extended syntax (`ere` true).
pub fn posix_to_rust(pat: &str, ere: bool) -> Option<String> {
    if pat.contains('\n') {
        // grep takes each line as a pattern of its own.
        return None;
    }
    let cs: Vec<char> = pat.chars().collect();
    let mut out = String::new();
    // Nothing yet, or an operator that starts a new alternative or group.
    let mut start = true;
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let mut next_start = false;
        match c {
            '\\' => {
                let d = *cs.get(i + 1)?;
                i += 1;
                match d {
                    '|' | '(' if !ere => {
                        out.push(d);
                        next_start = true;
                    }
                    ')' | '{' | '}' | '+' | '?' if !ere => out.push(d),
                    // Word boundaries and classes that Rust spells the same way.
                    'b' | 'B' | 'w' | 'W' | 's' | 'S' => {
                        out.push('\\');
                        out.push(d);
                    }
                    // Rust has no start-of-word or end-of-word anchor, so `\<` and `\>` are `\b`
                    // only next to a plain word character that no quantifier makes optional, where
                    // the two agree. Elsewhere, refuse.
                    '<' if begins_word(&cs, i + 1) => out.push_str("\\b"),
                    '>' if ends_word(&cs, i) => out.push_str("\\b"),
                    '<' | '>' | '`' | '\'' => return None,
                    d if d.is_ascii_punctuation() => {
                        out.push('\\');
                        out.push(d);
                    }
                    _ => return None,
                }
            }
            '[' => {
                i = bracket(&cs, i, &mut out)?;
            }
            '|' | '(' if ere => {
                out.push(c);
                next_start = true;
            }
            ')' | '{' | '}' | '+' | '?' if ere => out.push(c),
            '|' | '(' | ')' | '{' | '}' | '+' | '?' => {
                out.push('\\');
                out.push(c);
            }
            // A leading `*` of a basic expression is a plain star.
            '*' if !ere && start => out.push_str("\\*"),
            '^' if !ere => {
                // An anchor only at the start of a basic expression, literal elsewhere.
                if !start {
                    return None;
                }
                out.push(c);
                next_start = true;
            }
            '$' if !ere => {
                let rest: String = cs[i + 1..].iter().collect();
                if !(rest.is_empty() || rest.starts_with("\\)") || rest.starts_with("\\|")) {
                    return None;
                }
                out.push(c);
            }
            c => out.push(c),
        }
        start = next_start;
        i += 1;
    }
    Some(out)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Reports whether `cs[at]` is an unescaped word character. `\w`, `\W`, `\s` and `\b` are not.
fn plain_word(cs: &[char], at: usize) -> bool {
    is_word(cs[at]) && !(at >= 1 && cs[at - 1] == '\\')
}

/// Reports whether `\<` before `cs[at]` can be `\b`: a word character that no quantifier after it
/// makes optional, since `\<a*` also matches where no word starts.
fn begins_word(cs: &[char], at: usize) -> bool {
    cs.get(at).is_some_and(|&c| is_word(c)) && !may_be_empty(cs, at + 1)
}

/// Reports whether `\>` at `cs[at]` can be `\b`: a plain word character before it, and no
/// repetition after it.
fn ends_word(cs: &[char], at: usize) -> bool {
    at >= 2 && plain_word(cs, at - 2) && !repeats(cs, at + 1)
}

/// Reports whether a quantifier at `cs[at..]` can match none of the atom before it: `*`, `?`,
/// `\?`, or an interval that starts at zero.
fn may_be_empty(cs: &[char], at: usize) -> bool {
    let rest: String = cs.get(at..).unwrap_or(&[]).iter().take(4).collect();
    ["*", "?", "\\?", "\\{0", "{0", "\\{,", "{,"]
        .iter()
        .any(|q| rest.starts_with(*q))
}

/// Reports whether a repetition operator starts at `cs[at..]`, in basic or extended syntax.
fn repeats(cs: &[char], at: usize) -> bool {
    let rest: String = cs.get(at..).unwrap_or(&[]).iter().take(2).collect();
    ["*", "+", "?", "{", "\\+", "\\?", "\\{"]
        .iter()
        .any(|q| rest.starts_with(*q))
}

/// Copies the bracket expression at `cs[at]` into `out`; the index of its closing `]`.
fn bracket(cs: &[char], at: usize, out: &mut String) -> Option<usize> {
    out.push('[');
    let mut i = at + 1;
    if cs.get(i) == Some(&'^') {
        out.push('^');
        i += 1;
    }
    // A leading `]` is a member of the set.
    if cs.get(i) == Some(&']') {
        out.push_str("\\]");
        i += 1;
    }
    while let Some(&c) = cs.get(i) {
        match c {
            ']' => {
                out.push(']');
                return Some(i);
            }
            // POSIX has no escapes in a set; Rust does.
            '\\' => return None,
            '[' => match cs.get(i + 1) {
                Some(':') => {
                    let end = (i + 2..cs.len().saturating_sub(1))
                        .find(|&j| cs[j] == ':' && cs[j + 1] == ']')?;
                    let name: String = cs[i + 2..end].iter().collect();
                    if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase()) {
                        return None;
                    }
                    out.push_str(&format!("[:{name}:]"));
                    i = end + 1;
                }
                Some('.' | '=') => return None,
                _ => out.push_str("\\["),
            },
            '&' | '~' | '-' if cs.get(i + 1) == Some(&c) => return None,
            c => out.push(c),
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::posix_to_rust as p;

    #[test]
    fn translates_basic_and_extended_syntax() {
        for (pat, ere, want) in [
            ("foo", false, Some("foo")),
            ("foo\\|bar", false, Some("foo|bar")),
            ("foo|bar", false, Some("foo\\|bar")),
            ("foo|bar", true, Some("foo|bar")),
            ("a\\(b\\)\\{2\\}", false, Some("a(b){2}")),
            ("f(x)", false, Some("f\\(x\\)")),
            ("a+b?", false, Some("a\\+b\\?")),
            ("a\\+", false, Some("a+")),
            ("a\\.b", false, Some("a\\.b")),
            ("a\\.b", true, Some("a\\.b")),
            ("\\(", true, Some("\\(")),
            ("^## ", false, Some("^## ")),
            ("*x", false, Some("\\*x")),
            ("^*x", false, Some("^\\*x")),
            ("a\\|^b", false, Some("a|^b")),
            ("x$", false, Some("x$")),
            ("[[:alpha:]_]+", true, Some("[[:alpha:]_]+")),
            ("R1\\b\\|R12", false, Some("R1\\b|R12")),
            ("a\\s\\+b", false, Some("a\\s+b")),
            ("\\<foo\\>", false, Some("\\bfoo\\b")),
            ("\\<foo\\>", true, Some("\\bfoo\\b")),
            ("\\bfoo", true, Some("\\bfoo")),
            ("\\w+", true, Some("\\w+")),
            ("\\S\\W\\B", false, Some("\\S\\W\\B")),
            ("11\\.1[0-9]", false, Some("11\\.1[0-9]")),
            ("^## \\|^### ", false, Some("^## |^### ")),
            ("[]a]", false, Some("[\\]a]")),
            ("[^a-z[]", false, Some("[^a-z\\[]")),
            // Constructs whose meaning differs or is unknown.
            ("a^b", false, None),
            ("a$b", false, None),
            ("\\<", false, None),
            ("\\<-", false, None),
            ("\\<\\w", false, None),
            ("\\W\\>", false, None),
            ("a\\.\\>", false, None),
            ("\\1", false, None),
            ("[\\d]", false, None),
            ("[[.hyphen.]]", false, None),
            ("[a&&b]", true, None),
            ("[abc", false, None),
            ("a\nb", false, None),
            ("a\\", false, None),
        ] {
            assert_eq!(p(pat, ere).as_deref(), want, "{pat} ere={ere}");
        }
    }
}
