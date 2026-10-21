// SPDX-License-Identifier: MPL-2.0
//! Per-session state of the cancel filter: the held cancel and the open tool calls.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A `session/cancel` held back until it is known whether a prompt follows.
#[derive(Debug, Clone)]
pub struct PendingCancel {
    /// When the cancel arrived.
    pub at: Instant,
    /// The original line, forwarded as is if the window ends without a prompt.
    pub raw: Vec<u8>,
    /// The prompt the cancel was for, already answered as cancelled.
    pub prompt: Value,
}

/// A main-agent tool call (not a subagent's, not a subagent parent) that has
/// not reached a terminal status.
#[derive(Debug, Clone, Default)]
pub struct OpenTool {
    /// The prompt (as JSON text) that was newest when the call appeared.
    pub prompt: String,
    /// The input is still streaming: pending, and neither a status change nor
    /// a complete refinement has been seen.
    pub streaming: bool,
    pub title: String,
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Default)]
pub struct Session {
    /// Id of the outstanding `session/prompt` request, if any.
    pub prompt_id: Option<Value>,
    /// Parent tool calls whose subagents are running: seen a child update,
    /// and the parent has not completed or failed since.
    pub in_flight: HashSet<String>,
    /// Every Task/Agent parent seen this turn. Their own updates are not
    /// main-agent activity.
    pub parents: HashSet<String>,
    /// Last main-agent activity (or the prompt itself).
    pub last_main: Option<Instant>,
    /// Last main-agent text or thought chunk forwarded to the client.
    pub last_text: Option<Instant>,
    /// When the latest `session/prompt` arrived.
    pub turn_started: Option<Instant>,
    /// When the cancel filter last held a `session/cancel`.
    pub dropped_cancel: Option<Instant>,
    /// The cancel waiting for its decision, if a window is open.
    pub pending_cancel: Option<PendingCancel>,
    /// Open main-agent tool calls by id (ordered, for stable output).
    pub open_tools: BTreeMap<String, OpenTool>,
    /// Last session update of any kind (the prompt itself counts).
    pub last_activity: Option<Instant>,
    /// The latest main-agent update that says anything about the turn was
    /// assistant text or a usage update (not a tool call or a thought).
    pub final_text: bool,
    /// Ids (as JSON text) of agent requests (permissions, elicitations) the
    /// client has not answered yet.
    pub asking: HashSet<String>,
}

impl Session {
    /// Tells whether a prompt is outstanding and everything points to a turn that
    /// ended without its response: the last thing the agent said was text or
    /// a usage update, nothing is open (tool calls, subagents, questions to
    /// the client) and nothing at all arrived for `stall`. Zero never fires.
    pub fn looks_hung(&self, now: Instant, stall: Duration) -> bool {
        !stall.is_zero()
            && self.prompt_id.is_some()
            && self.in_flight.is_empty()
            && self.open_tools.is_empty()
            && self.asking.is_empty()
            && self.pending_cancel.is_none()
            && self.final_text
            && self
                .last_activity
                .is_some_and(|t| now.saturating_duration_since(t) >= stall)
    }

    /// Tells whether a tool call is still streaming its input.
    pub fn has_streaming_tool(&self) -> bool {
        self.open_tools.values().any(|t| t.streaming)
    }

    /// Tells whether a `session/cancel` now would only kill background subagents
    /// (or the turn holding them): a prompt is outstanding and subagents run.
    /// Whether the main agent is streaming does not matter.
    pub fn should_defer_cancel(&self) -> bool {
        self.prompt_id.is_some() && !self.in_flight.is_empty()
    }

    /// Tells whether the open decision window has lasted `defer` or longer, so no
    /// prompt followed the cancel and it was a real Stop.
    pub fn cancel_decision_due(&self, now: Instant, defer: Duration) -> bool {
        self.pending_cancel
            .as_ref()
            .is_some_and(|p| now.saturating_duration_since(p.at) >= defer)
    }

    /// Tells whether a previous cancel was held less than `window` ago, so this
    /// one is a deliberate Stop and must reach the agent. A zero window never
    /// matches.
    pub fn is_repeated_stop(&self, now: Instant, window: Duration) -> bool {
        self.dropped_cancel
            .is_some_and(|t| now.saturating_duration_since(t) < window)
    }

    /// Forgets everything turn-scoped when the turn ended, except the background
    /// subagents in `keep`, which still run.
    pub fn end_turn(&mut self, keep: &[String]) {
        self.in_flight.retain(|id| keep.contains(id));
        self.parents.retain(|id| keep.contains(id));
        self.open_tools.clear();
        self.asking.clear();
        self.final_text = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFER: Duration = Duration::from_millis(400);

    fn busy(t0: Instant) -> Session {
        Session {
            prompt_id: Some(Value::from(1)),
            in_flight: HashSet::from(["P".to_owned()]),
            parents: HashSet::from(["P".to_owned()]),
            last_main: Some(t0),
            ..Session::default()
        }
    }

    fn pending(t0: Instant) -> Session {
        Session {
            pending_cancel: Some(PendingCancel {
                at: t0,
                raw: Vec::new(),
                prompt: Value::from(1),
            }),
            ..busy(t0)
        }
    }

    #[test]
    fn defers_with_prompt_and_subagent_whatever_the_main_agent_does() {
        let t0 = Instant::now();
        // Just active, long quiet: the same answer.
        assert!(busy(t0).should_defer_cancel());
        let mut s = busy(t0);
        s.last_main = None;
        assert!(s.should_defer_cancel());
    }

    #[test]
    fn forwards_without_subagent_or_prompt() {
        let t0 = Instant::now();
        let mut s = busy(t0);
        s.in_flight.clear();
        assert!(!s.should_defer_cancel());
        let mut s = busy(t0);
        s.prompt_id = None;
        assert!(!s.should_defer_cancel());
    }

    #[test]
    fn decision_is_due_only_after_the_window() {
        let t0 = Instant::now();
        let s = pending(t0);
        assert!(!s.cancel_decision_due(t0, DEFER));
        assert!(!s.cancel_decision_due(t0 + Duration::from_millis(399), DEFER));
        assert!(s.cancel_decision_due(t0 + DEFER, DEFER));
        // No window open: never due.
        assert!(!busy(t0).cancel_decision_due(t0 + DEFER * 10, DEFER));
        // A zero window decides at the next look.
        assert!(s.cancel_decision_due(t0, Duration::ZERO));
    }

    #[test]
    fn repeated_stop_only_inside_window() {
        let t0 = Instant::now();
        let w = Duration::from_millis(3000);
        let mut s = busy(t0);
        assert!(!s.is_repeated_stop(t0, w));
        s.dropped_cancel = Some(t0);
        assert!(s.is_repeated_stop(t0 + Duration::from_millis(2999), w));
        assert!(!s.is_repeated_stop(t0 + w, w));
        assert!(!s.is_repeated_stop(t0, Duration::ZERO));
    }

    const STALL: Duration = Duration::from_secs(120);

    fn silent(t0: Instant) -> Session {
        Session {
            prompt_id: Some(Value::from(1)),
            final_text: true,
            last_activity: Some(t0),
            ..Session::default()
        }
    }

    #[test]
    fn looks_hung_only_after_the_whole_window_with_nothing_open() {
        let t0 = Instant::now();
        let s = silent(t0);
        assert!(!s.looks_hung(t0 + STALL - Duration::from_millis(1), STALL));
        assert!(s.looks_hung(t0 + STALL, STALL));
        assert!(!s.looks_hung(t0 + STALL, Duration::ZERO));

        let blockers: [fn(&mut Session); 6] = [
            |s| s.prompt_id = None,
            |s| {
                s.in_flight.insert("P".into());
            },
            |s| {
                s.open_tools.insert("m1".into(), OpenTool::default());
            },
            |s| {
                s.asking.insert("1".into());
            },
            |s| s.final_text = false,
            |s| s.last_activity = None,
        ];
        for block in blockers {
            let mut s = silent(t0);
            block(&mut s);
            assert!(!s.looks_hung(t0 + STALL * 10, STALL));
        }
        let mut s = pending(t0);
        s.in_flight.clear();
        s.final_text = true;
        assert!(!s.looks_hung(t0 + STALL * 10, STALL), "a held cancel");
    }

    #[test]
    fn a_streaming_tool_is_one_that_has_not_settled() {
        let mut s = Session::default();
        assert!(!s.has_streaming_tool());
        s.open_tools.insert(
            "a".into(),
            OpenTool {
                streaming: false,
                ..OpenTool::default()
            },
        );
        assert!(!s.has_streaming_tool());
        s.open_tools.insert(
            "b".into(),
            OpenTool {
                streaming: true,
                ..OpenTool::default()
            },
        );
        assert!(s.has_streaming_tool());
        s.end_turn(&[]);
        assert!(s.open_tools.is_empty() && !s.final_text);
    }

    #[test]
    fn end_turn_clears_subagents() {
        let t0 = Instant::now();
        let mut s = busy(t0);
        s.end_turn(&[]);
        assert!(!s.should_defer_cancel());
    }

    #[test]
    fn end_turn_keeps_background_subagents() {
        let t0 = Instant::now();
        let mut s = busy(t0);
        s.end_turn(&["P".to_owned()]);
        assert!(s.should_defer_cancel());
    }
}
