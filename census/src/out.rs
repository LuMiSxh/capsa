// SPDX-License-Identifier: MPL-2.0
//! Owns the output helpers: section headers, coloured only on a terminal.

use anstyle::{AnsiColor, Style};
use capsa_core::format::Format;
use std::io::IsTerminal;

/// Printing helpers. Colour is applied only when stdout is a terminal, so piped output is plain text.
pub struct Out {
    pub format: Format,
    color: bool,
}

impl Out {
    pub fn new(format: Format) -> Self {
        Out {
            format,
            color: format == Format::Human && std::io::stdout().is_terminal(),
        }
    }

    /// A section title, bold cyan on a terminal.
    pub fn header(&self, s: &str) -> String {
        if !self.color {
            return s.to_owned();
        }
        let style = Style::new().bold().fg_color(Some(AnsiColor::Cyan.into()));
        format!("{style}{s}{style:#}")
    }
}
