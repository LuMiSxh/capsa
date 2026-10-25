// SPDX-License-Identifier: MPL-2.0
//! Blanks dialect syntax that a tree-sitter grammar rejects but the language's own tools accept.

use std::borrow::Cow;

const ASM_QUALIFIERS: [&str; 4] = ["volatile", "__volatile__", "__volatile", "goto"];

/// Returns whether a JSON text uses comments or trailing commas, so it is JSONC rather than JSON.
pub fn is_jsonc(src: &str) -> bool {
    matches!(blank_jsonc(src), Cow::Owned(_))
}

/// Blanks the GNU `__attribute__((…))` and `__asm__(…)` groups of a C text. tree-sitter-c
/// reports an error on each declaration that carries one, which is not a mistake in the code.
pub(crate) fn blank_gnu_c(src: &str) -> Cow<'_, str> {
    let mut chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            i = quoted_end(&chars, i);
        } else if starts_with(&chars, i, "//") {
            i = line_end(&chars, i);
        } else if starts_with(&chars, i, "/*") {
            i = block_comment_end(&chars, i);
        } else if is_ident(c) && (i == 0 || !is_ident(chars[i - 1])) {
            let end = ident_end(&chars, i);
            i = match gnu_group_end(&chars, i, end) {
                Some(close) => {
                    blank(&mut chars, i, close);
                    close
                }
                None => end,
            };
        } else {
            i += 1;
        }
    }
    into_cow(src, chars)
}

/// Returns the end of the attribute or asm group that starts at `start`, or `None` when the
/// word at `start` is not one, or its parentheses do not close.
fn gnu_group_end(chars: &[char], start: usize, end: usize) -> Option<usize> {
    let keyword: String = chars[start..end].iter().collect();
    let mut i = skip_ws(chars, end);
    if matches!(keyword.as_str(), "__asm__" | "__asm" | "asm") {
        loop {
            let (word, next) = word_at(chars, i);
            if !ASM_QUALIFIERS.contains(&word.as_str()) {
                break;
            }
            i = skip_ws(chars, next);
        }
    } else if !matches!(keyword.as_str(), "__attribute__" | "__attribute") {
        return None;
    }
    if chars.get(i) != Some(&'(') {
        return None;
    }
    close_paren(chars, i)
}

/// Returns `src` with its comments and trailing commas blanked, so that tree-sitter-json reads
/// it as strict JSON with the same line numbers and columns.
pub(crate) fn blank_jsonc(src: &str) -> Cow<'_, str> {
    let mut chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let end = if chars[i] == '"' {
            quoted_end(&chars, i)
        } else if starts_with(&chars, i, "//") {
            let end = line_end(&chars, i);
            blank(&mut chars, i, end);
            end
        } else if starts_with(&chars, i, "/*") {
            let end = block_comment_end(&chars, i);
            blank(&mut chars, i, end);
            end
        } else {
            i + 1
        };
        i = end;
    }
    i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            i = quoted_end(&chars, i);
        } else {
            if chars[i] == ',' && matches!(chars.get(skip_ws(&chars, i + 1)), Some(&'}' | &']')) {
                chars[i] = ' ';
            }
            i += 1;
        }
    }
    into_cow(src, chars)
}

fn into_cow(src: &str, chars: Vec<char>) -> Cow<'_, str> {
    let text: String = chars.into_iter().collect();
    if text == src {
        Cow::Borrowed(src)
    } else {
        Cow::Owned(text)
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn ident_end(chars: &[char], start: usize) -> usize {
    let mut end = start;
    while chars.get(end).is_some_and(|&c| is_ident(c)) {
        end += 1;
    }
    end
}

/// Returns the identifier at `i` (empty when there is none) and the index after it.
fn word_at(chars: &[char], i: usize) -> (String, usize) {
    let end = ident_end(chars, i);
    (chars[i..end].iter().collect(), end)
}

fn skip_ws(chars: &[char], mut i: usize) -> usize {
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    i
}

fn starts_with(chars: &[char], i: usize, pat: &str) -> bool {
    pat.chars()
        .enumerate()
        .all(|(k, p)| chars.get(i + k) == Some(&p))
}

/// Returns the index after the string or character literal that opens at `start`. An
/// unterminated literal ends at its line.
fn quoted_end(chars: &[char], start: usize) -> usize {
    let quote = chars[start];
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '\n' => return i,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

fn line_end(chars: &[char], start: usize) -> usize {
    chars[start..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(chars.len(), |p| start + p)
}

fn block_comment_end(chars: &[char], start: usize) -> usize {
    let mut i = start + 2;
    while i + 1 < chars.len() {
        if chars[i] == '*' && chars[i + 1] == '/' {
            return i + 2;
        }
        i += 1;
    }
    chars.len()
}

/// Returns the index after the `)` that closes the `(` at `open`, or `None` when it never closes.
fn close_paren(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '"' | '\'' => {
                i = quoted_end(chars, i);
                continue;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Blanks `chars[from..to]` to spaces, keeping newlines so line numbers do not move.
fn blank(chars: &mut [char], from: usize, to: usize) {
    for c in &mut chars[from..to] {
        if *c != '\n' {
            *c = ' ';
        }
    }
}
