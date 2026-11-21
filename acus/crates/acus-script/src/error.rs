// SPDX-License-Identifier: MPL-2.0
//! Why a script run failed.

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
    },
    /// JavaScript ran past the deadline.
    #[error("script timed out after {secs}s")]
    Timeout { secs: u64 },
    /// The engine reached its memory limit.
    #[error("script exceeded the memory limit of {mib} MiB")]
    Memory { mib: usize },
    /// A saved module lacks something its caller requires.
    #[error("{0}")]
    Invalid(String),
    /// The engine failed outside the program: setup, or a value that has no JSON form.
    #[error("{0}")]
    Engine(String),
}
