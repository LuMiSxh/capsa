// SPDX-License-Identifier: MPL-2.0
//! Decides what the Claude Code hook tells the main agent: a line when the context or a limit
//! crosses a level it has not shown in this session. Returns values only; `census hook` prints.

mod levels;
mod state;

pub use state::log_line;

use crate::context;
use crate::limits::{LimitStatus, MonthStatus};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

/// The longest `additionalContext` the hook emits, in characters.
const MAX_CHARS: usize = 900;

/// Lists what can go wrong in the hook; every one means "print nothing".
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("the hook input is not JSON")]
    Input(#[from] serde_json::Error),
    #[error("cannot read or write the hint state")]
    State(#[from] std::io::Error),
}

/// The fields of the hook input that census reads. `agent_id` is set inside subagents only.
#[derive(Deserialize)]
struct Input {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    transcript_path: Option<PathBuf>,
    agent_id: Option<Value>,
}

/// Reads, decides and records the shown levels under the session's lock, so two concurrent calls
/// never print the same level. A call that cannot get the lock prints nothing.
fn announce(
    file: &std::path::Path,
    context: Option<&context::ContextUse>,
    limits: Option<&LimitStatus>,
    month: Option<&MonthStatus>,
    compacted: Option<(u64, u64)>,
    now: u64,
    prompt: bool,
) -> Result<Vec<String>, HookError> {
    let Some(_guard) = state::lock(file)? else {
        return Ok(Vec::new());
    };
    let mut shown = state::load(file)?;
    let before = shown.clone();
    let mut messages = Vec::new();
    if prompt {
        let due = levels::prompt_due(&mut shown, now);
        if due {
            messages.extend(levels::status_message(context, limits, month));
        }
    }
    messages.extend(levels::decide(
        context, limits, month, compacted, now, &mut shown,
    ));
    if shown != before {
        state::store(file, &shown)?;
    }
    Ok(messages)
}

/// Returns the hook output for `input`, the JSON text of `hookSpecificOutput`, or `None` when
/// there is nothing new to say: a subagent, another event, or no level crossed.
///
/// `limits` and `month` run only when the main agent is addressed, so the caller can bound their
/// time. The caller supplies the one that the mode needs and `None` from the other.
///
/// # Errors
/// Fails when the input is not JSON or the hint state cannot be read or written; the levels shown
/// are then not recorded and nothing is printed.
pub fn run(
    input: &str,
    now: u64,
    limits: impl FnOnce() -> Option<LimitStatus>,
    month: impl FnOnce() -> Option<MonthStatus>,
) -> Result<Option<String>, HookError> {
    let input: Input = serde_json::from_str(input)?;
    let event = match input.hook_event_name.as_deref() {
        Some(e @ ("UserPromptSubmit" | "PostToolUse")) if input.agent_id.is_none() => e,
        _ => return Ok(None),
    };
    let Some(file) = input.session_id.as_deref().and_then(state::hints_file) else {
        return Ok(None);
    };
    let transcript = input.transcript_path.as_deref();
    let context = transcript.and_then(context::of_transcript);
    let compacted = transcript.and_then(context::last_compaction);
    let limits = limits();
    let month = month();
    let messages = announce(
        &file,
        context.as_ref(),
        limits.as_ref(),
        month.as_ref(),
        compacted,
        now,
        event == "UserPromptSubmit",
    )?;
    if messages.is_empty() {
        return Ok(None);
    }
    let text: String = messages.join(" ").chars().take(MAX_CHARS).collect();
    let out = json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}});
    Ok(Some(out.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Threshold, at};
    use std::sync::Barrier;

    #[test]
    fn concurrent_calls_announce_a_level_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hints").join("s1");
        let threshold = Threshold {
            tokens: 100,
            source: "test".to_owned(),
        };
        let context = at(70, None, threshold);
        for _ in 0..20 {
            let _ = std::fs::remove_dir_all(dir.path().join("hints"));
            let barrier = Barrier::new(2);
            let sent: usize = std::thread::scope(|s| {
                let calls: Vec<_> = (0..2)
                    .map(|_| {
                        s.spawn(|| {
                            barrier.wait();
                            announce(&file, Some(&context), None, None, None, 1, false)
                                .unwrap()
                                .len()
                        })
                    })
                    .collect();
                calls.into_iter().map(|c| c.join().unwrap()).sum()
            });
            assert_eq!(sent, 1);
        }
    }
}
