// SPDX-License-Identifier: MPL-2.0
//! Owns `acus guard --replay FILE`: it decides each Bash call in FILE in-process, as the hook
//! does, and prints one JSON decision per line. guard.jsonl is never written, so a replay adds
//! nothing to `acus usage --guard`. A line is `{"tool","cwd","cmd"}` or guard.jsonl's own
//! `{"tool","cwd","call"}`. guard.jsonl cuts calls at 8000 characters, so a replayed cut call can
//! decide differently from the logged one.

use super::*;
use crate::log::LogEntry;

/// Why a replay stopped: the file, or one of its lines, could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// The file cannot be read; `file` is its path, as printed.
    #[error("{file}")]
    Read {
        file: String,
        #[source]
        source: std::io::Error,
    },
    /// A line is not a log entry.
    #[error("line {line}")]
    Parse {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    /// A line has neither `cmd` nor `call`.
    #[error("line {line}: no `cmd` or `call` field")]
    NoCommand { line: usize },
}

/// Names what the hook does with one Bash command.
pub(super) struct Decision {
    /// `rewrite`, `deny`, `hint`, `escape` or `pass`.
    pub(super) kind: &'static str,
    pub(super) rule: Option<&'static str>,
    pub(super) rewritten: Option<String>,
}

impl From<Option<Verdict>> for Decision {
    /// A verdict is the decision it gives; a command without one passes.
    fn from(verdict: Option<Verdict>) -> Self {
        match verdict {
            Some(v) => Decision {
                kind: v.kind(),
                rule: Some(v.rule().as_str()),
                rewritten: None,
            },
            None => Decision {
                kind: "pass",
                rule: None,
                rewritten: None,
            },
        }
    }
}

/// Returns the decision `run` takes for a Bash command: a rewrite first, then the checks.
pub(super) fn decide(cmd: &str, scope: Option<&Scope>) -> Decision {
    match decide_bash(cmd, scope, true) {
        BashDecision::Rewrite(rw) => Decision {
            kind: "rewrite",
            rule: Some(rw.rule.as_str()),
            rewritten: Some(rw.command),
        },
        BashDecision::Verdict(v) => Some(v).into(),
        BashDecision::Pass => None.into(),
    }
}

/// Prints the decision for each Bash line of `file`; lines of other tools are skipped.
pub fn replay_file(file: &Path, repeat: usize) -> Result<(), ReplayError> {
    let text = std::fs::read_to_string(file).map_err(|source| ReplayError::Read {
        file: file.display().to_string(),
        source,
    })?;
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let e: LogEntry = serde_json::from_str(line).map_err(|source| ReplayError::Parse {
            line: n + 1,
            source,
        })?;
        if e.tool.as_deref().is_some_and(|t| t != "Bash") {
            continue;
        }
        // guard.jsonl never logs an empty call, so an empty one counts as missing.
        let cmd = match e.cmd {
            Some(cmd) => cmd,
            None if !e.call.is_empty() => e.call,
            None => return Err(ReplayError::NoCommand { line: n + 1 }),
        };
        let scope = Scope::new(e.cwd.as_deref());
        for _ in 1..repeat {
            std::hint::black_box(decide(std::hint::black_box(&cmd), scope.as_ref()));
        }
        let d = decide(&cmd, scope.as_ref());
        let out = serde_json::json!({
            "cmd": cmd,
            "kind": d.kind,
            "rule": d.rule,
            "rewritten": d.rewritten,
        });
        println!("{out}");
    }
    Ok(())
}
