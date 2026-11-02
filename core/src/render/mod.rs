// SPDX-License-Identifier: MPL-2.0
//! Renders one result value as JSON, agent text or a human view built from a single Doc.

mod check;
mod doc;
mod paint;
#[cfg(test)]
mod tests;
mod text;

pub use check::{Check, Doctor, Group, Status};
pub use doc::{Align, Cell, Doc, Mark, Profile, Row, Table};
pub use paint::{Tone, bold, paint};

use std::io::{self, Write};

use serde::Serialize;

use crate::exit::Outcome;
use crate::format::{Format, Stream, color};

/// A result that renders as JSON, agent text and a human view.
pub trait Render: Serialize {
    /// Builds the human view. The agent view renders the same Doc in the plain profile.
    fn doc(&self, doc: &mut Doc);

    /// Writes the agent text, which overrides the default where the text is a tuned contract.
    fn agent(&self, out: &mut dyn Write) -> io::Result<()> {
        let mut doc = Doc::default();
        self.doc(&mut doc);
        doc.write(out, Profile::AGENT)
    }

    /// Returns the exit outcome of this result.
    fn outcome(&self) -> Outcome {
        Outcome::Found
    }
}

/// Writes `value` to stdout in `format` and returns its outcome. Colour follows stdout.
pub fn emit<R: Render + ?Sized>(value: &R, format: Format) -> anyhow::Result<Outcome> {
    let colour = color(format, Stream::Stdout);
    Ok(emit_to(value, format, colour, &mut io::stdout().lock())?)
}

/// Writes `value` to `out` in `format`: one compact JSON line, the human view (coloured when
/// `colour` is set) or the agent text.
pub fn emit_to<R: Render + ?Sized>(
    value: &R,
    format: Format,
    colour: bool,
    out: &mut dyn Write,
) -> io::Result<Outcome> {
    match format {
        Format::Json => {
            let line = serde_json::to_string(value)?;
            writeln!(out, "{line}")?;
        }
        Format::Agent => value.agent(out)?,
        Format::Human => {
            let mut doc = Doc::default();
            value.doc(&mut doc);
            doc.write(
                out,
                Profile {
                    color: colour,
                    rich: true,
                },
            )?;
        }
    }
    Ok(value.outcome())
}
