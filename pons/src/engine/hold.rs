// SPDX-License-Identifier: MPL-2.0
//! Held subagent entries, sent in finish order, and the handling of agent exit.

use super::*;

impl Engine {
    /// The agent process is gone. Every prompt still waiting for its response
    /// would never be answered, so each gets an error and the session one
    /// notice, instead of hanging as "working". `how` describes the exit.
    pub fn on_agent_exit(&mut self, how: &str) -> Vec<Action> {
        let text = format!("pons: the agent process exited ({how}); restart the thread");
        let mut waiting: Vec<(&String, &String)> =
            self.prompts.iter().map(|(k, s)| (s, k)).collect();
        waiting.sort();
        let mut out = Vec::new();
        let mut noticed: HashSet<&String> = HashSet::new();
        for (sid, key) in waiting {
            let Ok(id) = serde_json::from_str::<Value>(key) else {
                continue;
            };
            if noticed.insert(sid) {
                out.push(synthesized(
                    &json!({"jsonrpc": "2.0", "method": "session/update",
                    "params": {"sessionId": sid, "update": {"sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": format!("{text}.")}}}}),
                ));
            }
            out.push(Action::Note(format!(
                "agent exited ({how}) with prompt {id} of {sid} unanswered: answering it with an error"
            )));
            out.push(synthesized(&json!({"jsonrpc": "2.0", "id": id,
                "error": {"code": -32603, "message": text}})));
        }
        out
    }

    pub(super) fn release(&mut self, pick: impl Fn(&Held) -> bool, why: &str) -> Vec<Action> {
        let (out, keep): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|h| pick(h));
        self.held = keep;
        for h in &out {
            self.folds.mark_entry_sent(&h.parent);
        }
        out.iter()
            .flat_map(|h| {
                [
                    Action::Note(format!("finished entry for {} sent ({why})", h.parent)),
                    synthesized(&h.update),
                ]
            })
            .collect()
    }

    /// Sends the held entries whose session has had no main-agent text for
    /// `entry_gap` (or none at all).
    pub(super) fn release_due(&mut self, now: Instant) -> Vec<Action> {
        let gap = self.cfg.entry_gap;
        let quiet: HashSet<String> = self
            .held
            .iter()
            .filter(|h| {
                self.sessions
                    .get(&h.session)
                    .and_then(|s| s.last_text)
                    .is_none_or(|t| now.saturating_duration_since(t) >= gap)
            })
            .map(|h| h.session.clone())
            .collect();
        self.release(|h| quiet.contains(&h.session), "main agent text paused")
    }

    /// Sends every held entry of `sid` now, in finish order.
    pub(super) fn release_session(&mut self, sid: &str, why: &str) -> Vec<Action> {
        self.release(|h| h.session == sid, why)
    }
}
