// SPDX-License-Identifier: MPL-2.0
//! Owns the output format: agent (the default), human (coloured, on a terminal) or JSON.
//! It also decides whether colour is used and detects agent environments.

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::io::{self, IsTerminal};

/// How a tool renders its result and its errors: plain text for coding agents (the default),
/// plain text for people (coloured when [`color`] allows it), or one stable JSON document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Format {
    Agent,
    Json,
    Human,
}

/// Environment access, injectable so tests need no real environment.
pub trait Env {
    /// The value of `name`, or `None` when it is unset.
    fn var_os(&self, name: &str) -> Option<OsString>;

    /// Reports whether any variable name starts with `prefix`.
    fn any_with_prefix(&self, prefix: &str) -> bool;
}

#[derive(Debug, Clone, Copy)]
pub struct Process;

impl Env for Process {
    fn var_os(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }

    fn any_with_prefix(&self, prefix: &str) -> bool {
        std::env::vars_os().any(|(key, _)| key.as_encoded_bytes().starts_with(prefix.as_bytes()))
    }
}

/// Resolves the format from the command-line flags and the environment.
///
/// Explicit flags win, in the order `json`, `human`, `agent`. Without one,
/// `CAPSA_HUMAN=1` selects [`Format::Human`] unless an agent environment is
/// detected (see [`agent_env`]). The default is [`Format::Agent`].
pub fn resolve(human: bool, json: bool, agent: bool) -> Format {
    resolve_in(human, json, agent, &Process)
}

/// Resolves like [`resolve`], with an injected environment.
pub fn resolve_in(human: bool, json: bool, agent: bool, env: &dyn Env) -> Format {
    if json {
        Format::Json
    } else if human {
        Format::Human
    } else if agent {
        Format::Agent
    } else if env
        .var_os("CAPSA_HUMAN")
        .is_some_and(|value| value.to_str() == Some("1"))
        && !agent_env_in(env)
    {
        Format::Human
    } else {
        Format::Agent
    }
}

/// Reports whether an agent runs this process: `CLAUDECODE` is set, or a variable
/// starts with `CODEX_`.
pub fn agent_env() -> bool {
    agent_env_in(&Process)
}

/// Reports like [`agent_env`], with an injected environment.
pub fn agent_env_in(env: &dyn Env) -> bool {
    env.var_os("CLAUDECODE").is_some() || env.any_with_prefix("CODEX_")
}

/// Names an output stream a tool writes to; colour is decided per stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// Standard output: results.
    Stdout,
    /// Standard error: errors and diagnostics.
    Stderr,
}

impl Stream {
    pub fn is_terminal(self) -> bool {
        match self {
            Self::Stdout => io::stdout().is_terminal(),
            Self::Stderr => io::stderr().is_terminal(),
        }
    }
}

/// Reports whether text written to `stream` in `format` may use colour: human output,
/// the stream is a terminal, and `NO_COLOR` is unset or empty.
pub fn color(format: Format, stream: Stream) -> bool {
    color_in(format, stream.is_terminal(), &Process)
}

/// Reports whether stdout output in `format` may use colour, as [`color`] does.
pub fn color_stdout(format: Format) -> bool {
    color(format, Stream::Stdout)
}

/// Reports like [`color`], with the terminal state and the environment injected.
pub fn color_in(format: Format, terminal: bool, env: &dyn Env) -> bool {
    let no_color_set = env
        .var_os("NO_COLOR")
        .is_some_and(|value| !value.as_encoded_bytes().is_empty());
    format == Format::Human && terminal && !no_color_set
}

/// Formats an age in seconds as a compact relative time, such as `5m ago`.
///
/// Under a minute it reads `just now`. Otherwise it uses the largest unit that fits, truncated:
/// `m`, `h`, `d`, `mo` (30 days) or `y` (365 days).
pub fn ago(secs: u64) -> String {
    let (n, unit) = match secs {
        0..=59 => return "just now".to_owned(),
        60..=3599 => (secs / 60, "m"),
        3600..=86_399 => (secs / 3600, "h"),
        86_400..=2_591_999 => (secs / 86_400, "d"),
        2_592_000..=31_535_999 => (secs / 2_592_000, "mo"),
        _ => (secs / 31_536_000, "y"),
    };
    format!("{n}{unit} ago")
}
