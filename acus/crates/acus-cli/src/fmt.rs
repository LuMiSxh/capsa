// SPDX-License-Identifier: MPL-2.0
//! Output helpers shared by the commands: numbered lines, the head budget and clipping.

use capsa_core::format::{Format, Stream, color};
use capsa_core::render::{Doc, Profile, Tone, bold, paint};
use std::cell::RefCell;
use std::io;

thread_local! {
    /// Holds the output of the line `run` is executing instead of writing it to stdout.
    static CAPTURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Writes `s` to stdout, or into the buffer `capture` installed.
pub fn out(s: &str) {
    CAPTURE.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.as_mut() {
            Some(buf) => buf.push_str(s),
            None => print!("{s}"),
        }
    });
}

/// Runs `f` while `out` collects what it prints; returns the value and the collected text.
pub fn capture<T>(f: impl FnOnce() -> T) -> (T, String) {
    CAPTURE.with(|slot| *slot.borrow_mut() = Some(String::new()));
    let value = f();
    let text = CAPTURE.with(|slot| slot.borrow_mut().take().unwrap_or_default());
    (value, text)
}

/// Printing helpers. Colour follows `capsa_core::format::color`: human output on a terminal
/// without `NO_COLOR`, so piped and agent output is plain text.
pub struct Out {
    pub format: Format,
    color: bool,
}

impl Out {
    pub fn new(format: Format) -> Self {
        Out {
            format,
            color: color(format, Stream::Stdout),
        }
    }

    fn tint(&self, tone: Tone, s: &str) -> String {
        paint(s, tone, self.color).into_owned()
    }

    pub fn header(&self, s: &str) -> String {
        bold(s, self.color).into_owned()
    }

    pub fn dim(&self, s: &str) -> String {
        self.tint(Tone::Dim, s)
    }

    /// Writes a human-view `Doc` to stdout in the rich layout, coloured when colour is on.
    pub fn doc(&self, doc: &Doc) -> io::Result<()> {
        let mut rendered = Vec::new();
        doc.write(
            &mut rendered,
            Profile {
                color: self.color,
                rich: true,
            },
        )?;
        out(&String::from_utf8_lossy(&rendered));
        Ok(())
    }

    /// `n\tline`, or `n>\tline` for matching lines — the shape agents know from `cat -n`.
    pub fn numbered(&self, n: usize, line: &str, mark: bool) -> String {
        format!("{n}{}\t{line}", if mark { ">" } else { "" })
    }

    /// `n-\tline`, a context line next to `numbered`: grep marks context lines with `-`.
    pub fn context_line(&self, n: usize, text: &str) -> String {
        format!("{n}-\t{text}")
    }
}

/// Default cap on a printed line in chars; `find --cut N` sets another.
pub const CUT: usize = 200;

/// Trims leading whitespace and clips a hit line to `max` chars around its match (byte offset
/// `col`).
pub fn hit_text(s: &str, col: usize, max: usize) -> String {
    let t = s.trim_start();
    clip(t, col.saturating_sub(s.len() - t.len()), max)
}

/// Cap a line at `max` chars, keeping the window around byte offset `col`, so a minified file
/// costs a few hundred bytes per hit instead of the whole line.
pub fn clip(t: &str, col: usize, max: usize) -> String {
    if t.chars().nth(max).is_none() {
        return t.to_owned();
    }
    let at = t.get(..col).map_or(0, |p| p.chars().count());
    // Up to 60 chars before the match, fewer when the cap is small, so the match stays in view.
    let start = at.saturating_sub(60.min(max / 2));
    let body: String = t.chars().skip(start).take(max).collect();
    let rest = t.chars().count() - start - body.chars().count();
    let head = if start > 0 { "…" } else { "" };
    let tail = if rest > 0 {
        format!("… (+{rest} chars)")
    } else {
        String::new()
    };
    format!("{head}{body}{tail}")
}

/// The footer under a truncated listing, naming the command that shows the rest.
pub fn more_lines(path: &str, from: usize, to: usize, n: usize) -> String {
    format!("… {n} more lines (acus show {path}:{from}-{to})")
}

/// `--head N` budget: prints lines until N are out, then counts the rest for the footer.
pub struct Budget {
    head: Option<usize>,
    printed: usize,
    dropped: usize,
}

impl Budget {
    pub fn new(head: Option<usize>) -> Self {
        Budget {
            head,
            printed: 0,
            dropped: 0,
        }
    }

    /// Prints `text` line by line to stdout while under the head; counts the rest.
    pub fn emit(&mut self, text: &str) {
        for line in text.strip_suffix('\n').unwrap_or(text).split('\n') {
            if self.exhausted() {
                self.dropped += 1;
            } else {
                outln!("{line}");
                self.printed += 1;
            }
        }
    }

    pub fn exhausted(&self) -> bool {
        self.head.is_some_and(|n| self.printed >= n)
    }

    /// Names the rerun when lines were dropped. It is printed after the output, outside the head count.
    pub fn footer(&self) -> Option<String> {
        let n = self.head?;
        (self.dropped > 0).then(|| {
            format!(
                "… output cut at --head {n}; {} more lines (rerun with --head {})",
                self.dropped,
                n + self.dropped
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_lines_names_the_rest() {
        assert_eq!(
            more_lines("a.rs", 401, 900, 500),
            "… 500 more lines (acus show a.rs:401-900)"
        );
    }

    #[test]
    fn colour_is_applied_only_when_enabled() {
        let on = Out {
            format: Format::Human,
            color: true,
        };
        assert_eq!(on.header("x"), "\x1b[1mx\x1b[0m");
        assert_eq!(on.dim("y"), "\x1b[2my\x1b[0m");
        let off = Out {
            format: Format::Human,
            color: false,
        };
        assert_eq!(off.header("x"), "x");
        assert_eq!(off.dim("y"), "y");
    }

    #[test]
    fn context_line_has_a_dash() {
        let o = Out {
            format: Format::Agent,
            color: false,
        };
        assert_eq!(o.context_line(7, "x"), "7-\tx");
    }
}
