// SPDX-License-Identifier: MPL-2.0
//! Owns the hand-written splitter kept for PowerShell. It cuts at `;`, `|`, `&&`, `(` and the like
//! outside quotes and guesses nesting from keywords, which is why Bash moved to a parser.

use super::*;

/// Holds a simple command of a line, as a byte range of it.
pub(super) struct Span {
    pub(super) start: usize,
    pub(super) end: usize,
    /// Reports whether it reads a pipe.
    pub(super) piped: bool,
    /// Reports whether its stdout is data for something other than the agent: it sits inside
    /// `$( )`, backticks, `<( )`, a group or a compound command.
    pub(super) captured: bool,
}

/// Holds what a segment opens that a later one closes.
#[derive(PartialEq)]
enum Open {
    Paren,
    Backtick,
    /// `{`, `if`, `for`, `while`, `until`, `case` or `select`.
    Keyword,
}

/// Returns how many compound commands the segment `seg` opens and how many it closes, by the
/// keywords it begins with (`{ { cat a; }` opens two).
fn compound_keywords(seg: &str) -> (usize, usize) {
    let words = split_words(seg)
        .into_iter()
        .skip_while(|w| matches!(*w, "then" | "do" | "else" | "elif" | "!" | "time"));
    let (mut opens, mut closes) = (0, 0);
    for w in words {
        match w {
            "{" | "if" | "for" | "while" | "until" | "case" | "select" if closes == 0 => opens += 1,
            "}" | "fi" | "done" | "esac" if opens == 0 => closes += 1,
            _ => break,
        }
    }
    (opens, closes)
}

/// Returns the byte ranges of the simple commands of `cmd` outside quotes, each with whether it
/// reads a pipe. Ranges rather than strings, so a segment can be replaced in place.
pub(super) fn spans(cmd: &str, ps: bool) -> Vec<Span> {
    /// Ends the segment `start..end` and begins the next one at `next`. `opens` is what the
    /// segment opens, so it counts as captured, and `depth` the nesting when it began.
    fn cut(
        out: &mut Vec<Span>,
        (start, end): (&mut usize, &mut usize),
        piped: bool,
        next: usize,
        (stack, depth): (&mut Vec<Open>, &mut usize),
        text: &str,
        ps: bool,
    ) {
        // PowerShell closes its blocks with `}` alone, which is not tracked.
        let (opens, closes) = if ps {
            (0, 0)
        } else {
            compound_keywords(&text[*start..*end])
        };
        out.push(Span {
            start: *start,
            end: *end,
            piped,
            captured: *depth > 0 || !stack.is_empty() || opens > 0,
        });
        for _ in 0..closes {
            if stack.last() == Some(&Open::Keyword) {
                stack.pop();
            }
        }
        stack.extend((0..opens).map(|_| Open::Keyword));
        (*start, *end) = (next, next);
        *depth = stack.len();
    }
    /// Returns the end of the character after an escape, which is kept with it.
    fn escaped_end(chars: &mut impl Iterator<Item = (usize, char)>, fallback: usize) -> usize {
        chars.next().map_or(fallback, |(i, c)| i + c.len_utf8())
    }
    let mut out = Vec::new();
    let (mut stack, mut depth) = (Vec::new(), 0);
    // The segment read so far is `cmd[start..end]`.
    let (mut piped, mut start, mut end) = (false, 0, 0);
    let mut quote = None;
    let mut chars = cmd.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let after = at + c.len_utf8();
        match (quote, c) {
            // Inside double quotes an escaped quote does not end the string.
            (Some('"'), '\\') if !ps => end = escaped_end(&mut chars, after),
            (Some('"'), '`') if ps => end = escaped_end(&mut chars, after),
            (Some(q), _) => {
                if c == q {
                    quote = None;
                }
                end = after;
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                end = after;
            }
            // Backslash is a path separator in PowerShell, whose escape is the backtick.
            (None, '\\') if !ps => end = escaped_end(&mut chars, after),
            (None, '`') if ps => end = escaped_end(&mut chars, after),
            (None, '&')
                if cmd[start..end].ends_with('>') || chars.peek().map(|p| p.1) == Some('>') =>
            {
                end = after;
            }
            (None, '|') if chars.peek().map(|p| p.1) != Some('|') => {
                let st = (&mut stack, &mut depth);
                cut(&mut out, (&mut start, &mut end), piped, after, st, cmd, ps);
                piped = true;
            }
            (None, ';' | '\n' | '(' | ')' | '`' | '&' | '|') => {
                let next = if matches!(c, '&' | '|') {
                    chars.next_if(|p| p.1 == c).map_or(after, |p| p.0 + 1)
                } else {
                    after
                };
                let st = (&mut stack, &mut depth);
                cut(&mut out, (&mut start, &mut end), piped, next, st, cmd, ps);
                piped = false;
                // The nesting changes after the segment it cuts. A `)` that closes no `(` ends a
                // `case` pattern.
                match c {
                    '(' => stack.push(Open::Paren),
                    ')' if stack.last() == Some(&Open::Paren) => {
                        stack.pop();
                    }
                    '`' if !ps && stack.last() == Some(&Open::Backtick) => {
                        stack.pop();
                    }
                    '`' if !ps => stack.push(Open::Backtick),
                    _ => {}
                }
                depth = stack.len();
            }
            _ => end = after,
        }
    }
    let st = (&mut stack, &mut depth);
    cut(
        &mut out,
        (&mut start, &mut end),
        piped,
        cmd.len(),
        st,
        cmd,
        ps,
    );
    out
}

/// Reports whether `&&` or `||` comes right after the segment that ends at `end` in `text`.
pub(super) fn gates_at(text: &str, end: usize) -> bool {
    let rest = text[end..].trim_start();
    rest.starts_with("&&") || rest.starts_with("||")
}

/// Returns the segments of `cmd`, and the heredocs it opens.
pub(super) fn parse(cmd: &str, ps: bool) -> Parsed {
    let low = if ps {
        cmd.to_lowercase()
    } else {
        cmd.to_owned()
    };
    let (rest, docs) = strip_heredocs(&low);
    let shown = ps.then(|| strip_heredocs(cmd).0);
    let segs = segments(&rest, shown.as_deref().unwrap_or(&rest), ps);
    let docs = docs
        .into_iter()
        .map(|(opener, body)| Doc {
            openers: segments(&opener, &opener, ps)
                .into_iter()
                .map(|s| s.low)
                .collect(),
            body,
        })
        .collect();
    Parsed { segs, docs }
}

/// Returns the segments of `low` (what the rules read), each paired with its text in `orig`, the
/// same line in its original case. If `orig` splits into a different number of segments, `low`
/// stands in for it.
fn segments(low: &str, orig: &str, ps: bool) -> Vec<Segment> {
    let lows = spans(low, ps);
    let origs = (orig != low)
        .then(|| spans(orig, ps))
        .filter(|o| o.len() == lows.len());
    lows.iter()
        .enumerate()
        .map(|(i, sp)| {
            let (a, b) = (sp.start, sp.end);
            let text = &low[a..b];
            // The splitter cannot tell a group from a substitution, so both count as a group.
            let out = if redirects_stdout(&split_words(text)) {
                Out::File
            } else if lows.get(i + 1).is_some_and(|n| n.piped) {
                Out::Pipe
            } else {
                Out::Agent
            };
            let mut seg = Segment {
                low: quote_escaped_spaces(text),
                orig: text.to_owned(),
                range: 0..0,
                lead: 0,
                stdin: sp.piped,
                out,
                nest: if sp.captured { Nest::Group } else { Nest::Top },
                gates: gates_at(low, b),
                assigns: 0,
                heredoc: false,
            };
            if ps {
                seg.low = low[a..b].to_owned();
            }
            seg.orig = origs
                .as_ref()
                .map_or(&low[a..b], |o| &orig[o[i].start..o[i].end])
                .to_owned();
            seg
        })
        .collect()
}

/// Splits heredoc bodies, which are data rather than commands, from the rest of `cmd`; each body
/// comes with the line that opened it.
pub(super) fn strip_heredocs(cmd: &str) -> (String, Vec<(String, String)>) {
    // `<<<` is a here-string, not a heredoc.
    #[expect(clippy::unwrap_used, reason = "a constant pattern that the tests run")]
    static START: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:^|[^<])<<-?\s*['"]?([A-Za-z_][A-Za-z0-9_]*)['"]?"#).unwrap()
    });
    let mut out = String::new();
    let mut docs: Vec<(String, String)> = Vec::new();
    let mut end: Option<String> = None;
    for line in cmd.lines() {
        if let Some(d) = &end {
            if line.trim() == d {
                end = None;
            } else if let Some((_, body)) = docs.last_mut() {
                body.push_str(line);
                body.push('\n');
            }
            continue;
        }
        end = START.captures(line).map(|c| c[1].to_owned());
        if end.is_some() {
            docs.push((line.to_owned(), String::new()));
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, docs)
}
