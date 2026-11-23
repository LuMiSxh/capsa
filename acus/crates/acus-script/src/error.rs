// SPDX-License-Identifier: MPL-2.0
//! Why a script run failed, and what it produced before it did.

use serde_json::Value;

/// What a failed run produced before it failed, so the caller can show it.
#[derive(Debug, Default)]
pub struct Partial {
    /// The values passed to `print`, in order.
    pub prints: Vec<Value>,
    /// The values of host calls that finished without the program awaiting them, such as a
    /// `patch` that wrote before the program threw.
    pub completed: Vec<Value>,
}

/// A failed run, one variant per way the caller should explain it.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    /// The program has a syntax error or threw; `line` is the program's own line.
    #[error("{}{message}", line.map(|l| format!("line {l}: ")).unwrap_or_default())]
    Uncaught {
        line: Option<usize>,
        message: String,
        /// The `.hint` of a thrown host error.
        hint: Option<String>,
        partial: Partial,
    },
    /// JavaScript ran past the deadline.
    #[error("script timed out after {secs}s")]
    Timeout { secs: u64, partial: Partial },
    /// The engine reached its memory limit.
    #[error("script exceeded the memory limit of {mib} MiB")]
    Memory { mib: usize, partial: Partial },
    /// A saved module lacks something its caller requires.
    #[error("{0}")]
    Invalid(String),
    /// The engine failed outside the program: setup, or a value that has no JSON form.
    #[error("{0}")]
    Engine(String),
}

impl ScriptError {
    /// Attaches what the run produced before it failed; variants without a program keep none.
    pub(crate) fn with_partial(mut self, found: Partial) -> Self {
        if let Self::Uncaught { partial, .. }
        | Self::Timeout { partial, .. }
        | Self::Memory { partial, .. } = &mut self
        {
            *partial = found;
        }
        self
    }

    /// Returns what the run produced before it failed, or `None` when the failure had no program.
    pub fn partial(&self) -> Option<&Partial> {
        match self {
            Self::Uncaught { partial, .. }
            | Self::Timeout { partial, .. }
            | Self::Memory { partial, .. } => Some(partial),
            Self::Invalid(_) | Self::Engine(_) => None,
        }
    }
}
