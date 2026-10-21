// SPDX-License-Identifier: MPL-2.0
//! The cancel filter: holds `session/cancel` while subagents run and decides its fate.

use super::*;

impl Engine {
    /// Settles the held cancels whose decision window ended without a prompt:
    /// those were a real Stop.
    pub(super) fn decide_cancels(&mut self, now: Instant) -> Vec<Action> {
        let defer = self.cfg.cancel_defer;
        let mut due: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.cancel_decision_due(now, defer))
            .map(|(sid, _)| sid.clone())
            .collect();
        due.sort();
        let mut out = Vec::new();
        for sid in due {
            let Some(held) = self
                .sessions
                .get_mut(&sid)
                .and_then(|s| s.pending_cancel.take())
            else {
                continue;
            };
            out.push(Action::Note(format!(
                "stop: no prompt followed the held session/cancel for {sid} within {}ms; forwarding it to the agent",
                defer.as_millis()
            )));
            out.extend(self.finish_all(&sid, now));
            out.extend(self.close_ghosts(&sid, None, "stop"));
            out.extend(self.end_turn(&sid, now, false));
            if let Some(s) = self.sessions.get_mut(&sid) {
                s.dropped_cancel = None;
            }
            out.push(Action::ToAgent(held.raw));
        }
        out
    }

    /// Fails the open main-agent tool calls of a cancelled turn, once: `only`
    /// limits it to the calls that appeared under that prompt.
    pub(super) fn close_ghosts(&mut self, sid: &str, only: Option<&str>, why: &str) -> Vec<Action> {
        if !self.cfg.close_ghost_calls || self.aside {
            return Vec::new();
        }
        let Some(session) = self.sessions.get_mut(sid) else {
            return Vec::new();
        };
        let ids: Vec<String> = session
            .open_tools
            .iter()
            .filter(|(_, t)| only.is_none_or(|p| t.prompt == p))
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        for id in &ids {
            session.open_tools.remove(id);
        }
        let mut out = vec![Action::Note(format!(
            "closing {} open tool call(s) of {sid} as failed ({why}): {}",
            ids.len(),
            ids.join(", ")
        ))];
        out.extend(ids.iter().map(|id| {
            synthesized(&json!({"jsonrpc": "2.0", "method": "session/update",
                "params": {"sessionId": sid, "update": {"sessionUpdate": "tool_call_update",
                    "toolCallId": id, "status": "failed"}}}))
        }));
        out
    }

    /// Decides a `session/cancel`; `Some` when the cancel is dropped.
    ///
    /// Cancel and message-send, per session with subagents in flight:
    ///
    /// ```text
    /// idle --cancel--> WINDOW   prompt answered "cancelled" at once, its id
    ///                           swallowed; the cancel is held
    /// WINDOW --cancel again (within --stop-window-ms)--> idle
    ///                           forwarded at once (stop escape)
    /// WINDOW --prompt--> SEND   the cancel is dropped for good
    /// WINDOW --window over (--cancel-defer-ms)--> idle
    ///                           real Stop: cards stopped, cancel forwarded
    /// SEND, steer: `_session/steering` goes to the agent instead of the
    ///   prompt (AWAIT). Reply `injected`: the old prompt's turn now carries
    ///   the message and the new prompt id waits (HANDOFF); when the agent
    ///   answers the old prompt, pons answers the new one with that result.
    ///   Any other reply or an error: the prompt is forwarded (queue).
    /// SEND, queue: the prompt is forwarded; the adapter queues it behind
    ///   the running turn and answers the old prompt first (swallowed).
    /// ```
    ///
    /// `Some` when the cancel is held or dropped, `None` when it is forwarded.
    pub(super) fn on_cancel(
        &mut self,
        msg: &Value,
        raw: &[u8],
        now: Instant,
    ) -> Option<Vec<Action>> {
        if self.cfg.interrupt == Some(InterruptMode::Cancel) {
            return None;
        }
        let sid = session_id(msg)?;
        let session = self.sessions.get_mut(sid)?;
        if session.pending_cancel.is_some() {
            if session.is_repeated_stop(now, self.cfg.stop_window) {
                return None;
            }
            return Some(vec![Action::Note(format!(
                "cancel for {sid} dropped: one is already held (--stop-window-ms 0)"
            ))]);
        }
        if !session.should_defer_cancel() {
            return None;
        }
        let id = session.prompt_id.take()?;
        session.dropped_cancel = Some(now);
        session.pending_cancel = Some(PendingCancel {
            at: now,
            raw: raw.to_vec(),
            prompt: id.clone(),
        });
        let note = format!(
            "cancel deferred: holding session/cancel for {sid} for {}ms to tell Stop from a message; answering prompt {id} as cancelled ({} subagent(s) in flight)",
            self.cfg.cancel_defer.as_millis(),
            session.in_flight.len()
        );
        let key = id.to_string();
        self.prompts.remove(&key);
        self.swallow.insert(key, sid.to_owned());
        let response = json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "cancelled"}});
        let mut out = vec![Action::Note(note)];
        out.extend(self.release_session(sid, "prompt answered as cancelled"));
        out.push(synthesized(&response));
        Some(out)
    }

    /// Handles a cancel that reaches the agent. If it repeats a dropped one within
    /// the stop window it is a real Stop: reset in-flight tracking, since the
    /// agent is about to end the turn. The swallow entry of the earlier
    /// prompt stays; its response is still owed and must stay hidden from the
    /// client, which already got a synthesized one.
    pub(super) fn on_cancel_forwarded(
        &mut self,
        msg: &Value,
        raw: &[u8],
        now: Instant,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(sid) = session_id(msg)
            && let Some(session) = self.sessions.get_mut(sid)
        {
            let stop = session.is_repeated_stop(now, self.cfg.stop_window);
            session.dropped_cancel = None;
            session.pending_cancel = None;
            if stop {
                out.push(Action::Note(format!(
                    "stop escape: forwarding repeated session/cancel for {sid} ({} subagent(s) in flight)",
                    session.in_flight.len()
                )));
                out.extend(self.finish_all(sid, now));
                out.extend(self.close_ghosts(sid, None, "stop escape"));
                out.extend(self.end_turn(sid, now, false));
            }
        }
        out.push(Action::ToAgent(raw.to_vec()));
        out
    }
}
