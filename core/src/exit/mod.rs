// SPDX-License-Identifier: MPL-2.0
//! Owns the exit codes and the rendering of errors, the same in every output format.
//! It also holds the crate's only `unsafe` code, the SIGPIPE reset.

#[cfg(test)]
mod tests;

use std::io::{self, Write};
use std::process::ExitCode;

use crate::format::{Format, Stream, color};
use crate::hint::find_hint;

const ERROR_CODE: u8 = 2;

/// Maps what a successful run found to an exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Something was found: exit 0.
    Found,
    /// Nothing was found: exit 1. This is not a failure.
    Empty,
    /// Any other code, chosen by the tool.
    Code(u8),
}

impl Outcome {
    /// Returns the process exit code for this outcome.
    pub fn code(self) -> u8 {
        match self {
            Self::Found => 0,
            Self::Empty => 1,
            Self::Code(code) => code,
        }
    }
}

/// Turns a command's result into its exit code, rendering any error.
pub fn finish(result: anyhow::Result<Outcome>, format: Format) -> ExitCode {
    match result {
        Ok(outcome) => ExitCode::from(outcome.code()),
        Err(err) => report(&err, format),
    }
}

/// Prints `err` to stderr in `format` and returns exit code 2.
pub fn report(err: &anyhow::Error, format: Format) -> ExitCode {
    // Colour follows stderr, the stream written here. A failed write leaves
    // nothing to report it to, so the error is dropped.
    let colour = color(format, Stream::Stderr);
    let _ = write_error(&mut io::stderr().lock(), err, format, colour);
    ExitCode::from(ERROR_CODE)
}

/// Writes `err` to `out`: `error:`, then `hint:` when there is one. Human
/// output adds one `  caused by:` line per further cause; agent output puts
/// the chain on the `error:` line. JSON gets one line instead.
pub fn write_error(
    out: &mut impl Write,
    err: &anyhow::Error,
    format: Format,
    color: bool,
) -> io::Result<()> {
    match format {
        Format::Json => write_json(out, err),
        Format::Agent | Format::Human => write_text(out, err, format, color),
    }
}

fn write_text(
    out: &mut impl Write,
    err: &anyhow::Error,
    format: Format,
    color: bool,
) -> io::Result<()> {
    let (red, cyan, reset) = if color {
        ("\x1b[31m", "\x1b[36m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    if format == Format::Human {
        writeln!(out, "{red}error:{reset} {err}")?;
        for cause in err.chain().skip(1) {
            writeln!(out, "  caused by: {cause}")?;
        }
    } else {
        // Agent output puts the whole chain on one line, as `error: top: cause: cause`.
        writeln!(out, "{red}error:{reset} {err:#}")?;
    }
    if let Some(hint) = find_hint(err) {
        writeln!(out, "{cyan}hint:{reset} {hint}")?;
    }
    Ok(())
}

fn write_json(out: &mut impl Write, err: &anyhow::Error) -> io::Result<()> {
    let causes: Vec<String> = err.chain().skip(1).map(ToString::to_string).collect();
    let line = serde_json::json!({
        "error": err.to_string(),
        "causes": causes,
        "hint": find_hint(err),
    });
    writeln!(out, "{line}")
}

/// Restores the default SIGPIPE action, so a pipe into `head` ends the tool
/// quietly. Rust ignores SIGPIPE at startup; call this first in `main`.
#[cfg(unix)]
pub fn reset_sigpipe() {
    // SAFETY: `signal` only sets the disposition of SIGPIPE to SIG_DFL, a
    // valid handler constant. It takes no pointers and runs once at startup.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Does nothing on platforms without SIGPIPE.
#[cfg(not(unix))]
pub fn reset_sigpipe() {}
