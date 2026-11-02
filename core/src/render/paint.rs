// SPDX-License-Identifier: MPL-2.0
//! Owns the colour of rendered text: the tones and the ANSI codes that carry them.

use std::borrow::Cow;

/// A colour role a piece of text plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Fail,
    Accent,
    Dim,
    Plain,
}

impl Tone {
    /// Returns the tone of a used fraction: below 75% is `Ok`, below 90% `Warn`, else `Fail`.
    pub fn for_ratio(ratio: f64) -> Self {
        if ratio < 0.75 {
            Self::Ok
        } else if ratio < 0.9 {
            Self::Warn
        } else {
            Self::Fail
        }
    }

    fn code(self) -> Option<&'static str> {
        match self {
            Self::Ok => Some("32"),
            Self::Warn => Some("33"),
            Self::Fail => Some("31"),
            Self::Accent => Some("36"),
            Self::Dim => Some("2"),
            Self::Plain => None,
        }
    }
}

/// Wraps `s` in the ANSI code of `tone` when `color` is set, and returns it unchanged otherwise.
pub fn paint(s: &str, tone: Tone, color: bool) -> Cow<'_, str> {
    match tone.code() {
        Some(code) if color => Cow::Owned(format!("\x1b[{code}m{s}\x1b[0m")),
        _ => Cow::Borrowed(s),
    }
}

/// Makes `s` bold when `color` is set, for titles and section names.
pub fn bold(s: &str, color: bool) -> Cow<'_, str> {
    if color {
        Cow::Owned(format!("\x1b[1m{s}\x1b[0m"))
    } else {
        Cow::Borrowed(s)
    }
}
