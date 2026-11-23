// SPDX-License-Identifier: MPL-2.0
//! Everything a command prints: stdout, colour, the `--head`
//! budget, clipping and the footers that name the command to rerun.

use capsa_core::format::{Format, Stream, color};
use capsa_core::render::{Doc, Profile, bold};

use std::cell::RefCell;
use std::io;

thread_local! {
    static CAPTURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Prints one line through `out::write`.
macro_rules! outln {
   () => { $crate::out::write("\n") };
   ($($arg:tt)*) => {{
       $crate::out::write(&format!("{}\n", format_args!($($arg)*)));
   }};
}

/// Prints without a newline through `out::write`, as `print!` does.
macro_rules! outprint {
   ($($arg:tt)*) => {{
       $crate::out::write(&format!($($arg)*));
   }};
}

mod budget;
mod clip;

pub use budget::Budget;
pub use clip::{CUT, clip, clip_lines, fit_line, fit_lines, hit_text, more_lines};

/// Writes `s` to stdout, or to the buffer of the running [`capture`] on this thread.
pub fn write(s: &str) {
    let kept = CAPTURE.with(|c| {
        c.borrow_mut()
            .as_mut()
            .map(|buffer| buffer.push_str(s))
            .is_some()
    });
    if !kept {
        print!("{s}");
    }
}

/// Runs `print` and returns what it wrote through [`write`] instead of printing it.
#[cfg(feature = "script")]
pub fn capture(print: impl FnOnce()) -> String {
    CAPTURE.with(|c| *c.borrow_mut() = Some(String::new()));
    print();
    CAPTURE.with(|c| c.borrow_mut().take()).unwrap_or_default()
}

/// Printing helpers. Colour follows `capsa_core::format::color`: human output on a terminal
/// without `NO_COLOR`, so piped and agent output is plain text.
pub struct Out {
    pub format: Format,
    pub color: bool,
}

impl Out {
    pub fn new(format: Format) -> Self {
        Out {
            format,
            color: color(format, Stream::Stdout),
        }
    }

    pub fn header(&self, s: &str) -> String {
        bold(s, self.color).into_owned()
    }

    /// Writes a human-view `Doc` within `budget`, line by line.
    pub fn doc_budget(&self, doc: &Doc, budget: &mut Budget) -> io::Result<()> {
        let mut rendered = Vec::new();
        doc.write(
            &mut rendered,
            Profile {
                color: self.color,
                rich: true,
            },
        )?;
        budget.emit(String::from_utf8_lossy(&rendered).trim_end_matches('\n'));
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

#[cfg(test)]
mod tests {
    use super::*;
    use capsa_core::render::{Tone, paint};

    #[test]
    fn colour_is_applied_only_when_enabled() {
        let on = Out {
            format: Format::Human,
            color: true,
        };
        assert_eq!(on.header("x"), "\x1b[1mx\x1b[0m");
        assert_eq!(paint("y", Tone::Dim, on.color), "\x1b[2my\x1b[0m");
        let off = Out {
            format: Format::Human,
            color: false,
        };
        assert_eq!(off.header("x"), "x");
        assert_eq!(paint("y", Tone::Dim, off.color), "y");
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
