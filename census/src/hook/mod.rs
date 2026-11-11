// SPDX-License-Identifier: MPL-2.0
//! Decides what the Claude Code hook tells the main agent: a line when the context or a limit
//! crosses a level it has not shown in this session. Returns values only; `census hook` prints.

mod levels;
mod state;

pub use state::{ErrorLine, error_log, log_error};

use crate::config::Config;
use crate::context;
use crate::limits::{DailyStatus, LimitStatus, MonthStatus};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::json;
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
    agent_id: Option<IgnoredAny>,
}

/// Reads, decides and records the shown levels under the session's lock, so two concurrent calls
/// never print the same level. A call that cannot get the lock prints nothing. `Opening::Session`
/// adds the usage-history hint the first time the session sees it.
/// The budget the hook announces from: the month, today's daily limit when one applies, and the
/// daily thresholds in percent.
pub(super) struct Budget<'a> {
    pub month: &'a MonthStatus,
    pub daily: Option<DailyStatus>,
    pub thresholds: &'a [u32],
}

/// What a call adds besides the level messages.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Opening {
    /// The first prompt of a session may add the status line.
    Prompt,
    /// A session start adds the usage-history hint.
    Session,
    /// A tool call adds only the level messages.
    Tool,
}

fn announce(
    file: &std::path::Path,
    context: Option<&context::ContextUse>,
    limits: Option<&LimitStatus>,
    budget: Option<&Budget>,
    compacted: Option<(u64, u64)>,
    now: u64,
    opening: Opening,
) -> Result<Vec<String>, HookError> {
    let Some(_guard) = state::lock(file)? else {
        return Ok(Vec::new());
    };
    let mut shown = state::load(file)?;
    let before = shown.clone();
    let mut messages = Vec::new();
    if opening == Opening::Session {
        messages.extend(levels::usage_hint(&mut shown));
    }
    if opening == Opening::Prompt {
        let due = levels::prompt_due(&mut shown, now);
        if due {
            messages.extend(levels::status_message(
                context,
                limits,
                budget.map(|b| b.month),
            ));
        }
    }
    messages.extend(levels::decide(
        context, limits, budget, compacted, now, &mut shown,
    ));
    if shown != before {
        state::store(file, &shown)?;
    }
    Ok(messages)
}

/// Returns the hook output for `input`, the JSON text of `hookSpecificOutput`, or `None` when
/// there is nothing new to say: a subagent, another event, or no level crossed.
///
/// `SessionStart` points at the report commands once per session. `limits` and `month` run only
/// for the other events, so the caller can bound their time. The caller supplies the one that the
/// mode needs and `None` from the other.
///
/// # Errors
/// Fails when the input is not JSON or the hint state cannot be read or written; the levels shown
/// are then not recorded and nothing is printed.
pub fn run(
    input: &str,
    now: u64,
    config: &Config,
    limits: impl FnOnce() -> Option<LimitStatus>,
    month: impl FnOnce() -> Option<MonthStatus>,
) -> Result<Option<String>, HookError> {
    let input: Input = serde_json::from_str(input)?;
    let event = match input.hook_event_name.as_deref() {
        Some(e @ ("SessionStart" | "UserPromptSubmit" | "PostToolUse"))
            if input.agent_id.is_none() =>
        {
            e
        }
        _ => return Ok(None),
    };
    let Some(file) = input.session_id.as_deref().and_then(state::hints_file) else {
        return Ok(None);
    };
    if event == "SessionStart" {
        let messages = announce(&file, None, None, None, None, now, Opening::Session)?;
        return Ok(output(event, &messages));
    }
    let ends = input
        .transcript_path
        .as_deref()
        .map(context::transcript_ends)
        .unwrap_or_default();
    // The settings file is read once per call, for the threshold of the context.
    let settings = context::settings_window(&context::claude_config_dir());
    let context = ends
        .context
        .map(|(model, tokens)| context::measure_with(tokens, Some(&model), settings));
    let compacted = ends.compacted;
    let limits = limits();
    let month = month();
    let budget = month.as_ref().map(|m| Budget {
        month: m,
        daily: DailyStatus::of(m, config.daily_limit_usd),
        thresholds: &config.daily_thresholds,
    });
    let opening = if event == "UserPromptSubmit" {
        Opening::Prompt
    } else {
        Opening::Tool
    };
    let messages = announce(
        &file,
        context.as_ref(),
        limits.as_ref(),
        budget.as_ref(),
        compacted,
        now,
        opening,
    )?;
    Ok(output(event, &messages))
}

/// Returns the hook output for `messages`, or `None` when there is nothing to say.
fn output(event: &str, messages: &[String]) -> Option<String> {
    if messages.is_empty() {
        return None;
    }
    let text: String = messages.join(" ").chars().take(MAX_CHARS).collect();
    let out = json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}});
    Some(out.to_string())
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
                            announce(&file, Some(&context), None, None, None, 1, Opening::Tool)
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
