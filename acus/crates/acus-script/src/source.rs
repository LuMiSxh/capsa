// SPDX-License-Identifier: MPL-2.0
//! Makes the last expression of a program its result, for a program that neither returns nor
//! prints by itself.

use rquickjs::{Context, Ctx, Function, Runtime, Value};
use std::borrow::Cow;

/// Words that open a statement, which cannot be the result of a program.
const STATEMENT_WORDS: &[&str] = &[
    "async", "break", "class", "const", "continue", "debugger", "do", "export", "for", "function",
    "if", "import", "let", "return", "switch", "throw", "try", "var", "while", "with",
];

/// What a scan of a program found.
struct Scan {
    /// The top-level statement starts, in order.
    starts: Vec<Start>,
    /// Whether the program names `return` or `print` itself, so its result is its own.
    explicit: bool,
}

/// A byte offset where a top-level statement may start.
struct Start {
    at: usize,
    /// The last token before `at`: a word or number is `b'a'`, a string is `b'"'`, and `;` is
    /// `b';'`. It is `0` at the start of the program.
    after: u8,
}

/// Returns `code` with `return` in front of its last top-level expression, or `code` itself when
/// the program returns or prints, or when no such expression parses.
///
/// Each candidate is checked by compiling it as an async function body, which parses without
/// running it. The scan is a light lexer: a regular expression literal that holds a bracket can
/// make it see the wrong depth, and the parse check then decides.
pub(crate) fn implicit_return(code: &str) -> Cow<'_, str> {
    let scan = scan(code);
    if scan.explicit {
        return Cow::Borrowed(code);
    }
    let Ok(runtime) = Runtime::new() else {
        return Cow::Borrowed(code);
    };
    let Ok(context) = Context::full(&runtime) else {
        return Cow::Borrowed(code);
    };
    context
        .with(|ctx| last_expression(&ctx, code, &scan.starts))
        .map_or(Cow::Borrowed(code), Cow::Owned)
}

/// Returns the program with `return` before the latest candidate that is a top-level statement of
/// its own. A candidate is skipped when it continues the expression before it, or when the program
/// before it does not parse alone, as with a brace-less loop body or a label. It stops at a
/// statement that cannot be a result, so a declaration is never returned.
fn last_expression(ctx: &Ctx<'_>, code: &str, starts: &[Start]) -> Option<String> {
    let body: Function = ctx.eval("(async () => {}).constructor").ok()?;
    for start in starts.iter().rev() {
        let line = code[start.at..]
            .lines()
            .next()
            .unwrap_or_default()
            .trim_start();
        if line.is_empty() || line.starts_with("//") || line.starts_with("/*") {
            continue;
        }
        let word = line
            .split(|c: char| !c.is_ascii_alphanumeric())
            .next()
            .unwrap_or_default();
        if STATEMENT_WORDS.contains(&word) || line.starts_with(['{', '}', ';']) {
            return None;
        }
        if continues_expression(start.after, line) {
            continue;
        }
        let candidate = format!("{}return {}", &code[..start.at], &code[start.at..]);
        if parses(ctx, &body, &candidate) && parses(ctx, &body, &code[..start.at]) {
            return Some(candidate);
        }
    }
    None
}

/// Returns whether `text` parses as an async function body. The body is compiled, never run.
fn parses(ctx: &Ctx<'_>, body: &Function<'_>, text: &str) -> bool {
    if body.call::<_, Value>((text,)).is_ok() {
        return true;
    }
    // Clears the SyntaxError the failed parse left behind.
    let _ = ctx.catch();
    false
}

/// Returns whether `line`, which follows a token `after`, continues that token's expression. A
/// line break ends a statement only where the grammar cannot go on, so `x = y` then `+ 5` is one
/// expression, while `++` always begins a new one.
fn continues_expression(after: u8, line: &str) -> bool {
    matches!(after, b'a' | b'"' | b')' | b']' | b'}')
        && !line.starts_with("++")
        && !line.starts_with("--")
        && line.starts_with(|c: char| "+-*/%&|^<>=?:,.([`".contains(c))
}

/// Lexes `code` just far enough to find its top-level statement starts and its own `return` or
/// `print`. Strings, template literals and comments are skipped.
fn scan(code: &str) -> Scan {
    let bytes = code.as_bytes();
    let mut scan = Scan {
        starts: vec![Start { at: 0, after: 0 }],
        explicit: false,
    };
    let mut depth = 0_usize;
    let mut last = 0_u8;
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let next = bytes.get(i + 1).copied();
        i = match (b, next) {
            (b'/', Some(b'/')) => find_from(bytes, i, b'\n'),
            (b'/', Some(b'*')) => block_comment_end(bytes, i),
            (b'\'' | b'"' | b'`', _) => {
                last = b'"';
                quote_end(bytes, i)
            }
            (b'(' | b'[' | b'{', _) => {
                depth += 1;
                last = b;
                i + 1
            }
            (b')' | b']' | b'}', _) => {
                depth = depth.saturating_sub(1);
                last = b;
                i + 1
            }
            (b';' | b'\n', _) if depth == 0 => {
                if b == b';' {
                    last = b;
                }
                scan.starts.push(Start {
                    at: i + 1,
                    after: last,
                });
                i + 1
            }
            _ if is_word(b) => {
                let end = word_end(bytes, i);
                if matches!(&code[i..end], "return" | "print") {
                    scan.explicit = true;
                }
                last = b'a';
                end
            }
            _ => {
                if !b.is_ascii_whitespace() {
                    last = b;
                }
                i + 1
            }
        };
    }
    scan
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$') || b >= 0x80
}

fn word_end(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|&b| !is_word(b))
        .map_or(bytes.len(), |n| start + n)
}

/// Returns the index of the first `byte` at or after `from`, or the end of `bytes`.
fn find_from(bytes: &[u8], from: usize, byte: u8) -> usize {
    bytes[from..]
        .iter()
        .position(|&b| b == byte)
        .map_or(bytes.len(), |n| from + n)
}

fn block_comment_end(bytes: &[u8], start: usize) -> usize {
    bytes[start + 2..]
        .windows(2)
        .position(|w| w == b"*/")
        .map_or(bytes.len(), |n| start + 2 + n + 2)
}

/// Returns the index just past the string or template literal that opens at `start`. A quoted
/// string ends at its line's end when it is not closed.
fn quote_end(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut j = start + 1;
    while let Some(&b) = bytes.get(j) {
        match b {
            b'\\' => j += 2,
            b'\n' if quote != b'`' => return j,
            _ if b == quote => return j + 1,
            _ => j += 1,
        }
    }
    bytes.len()
}
