// SPDX-License-Identifier: MPL-2.0
//! Timed work: the periodic tick and the turn watchdog.

use super::*;

impl Engine {
    pub(super) fn tick_rules(&mut self, now: Instant) -> Vec<Action> {
        let mut out = self.decide_cancels(now);
        out.extend(self.release_steers_due(now));
        out.extend(self.watchdog(now));
        out.extend(
            self.notes
                .flush_stale(now)
                .into_iter()
                .map(|line| Action::ToClient {
                    line,
                    synthesized: false,
                }),
        );
        let finished = self.folds.poll(now);
        out.extend(self.finished(finished));
        out.extend(self.fold_notes());
        out.extend(self.release_due(now));
        out.extend(self.folds.tick(now).iter().map(synthesized));
        let gone = self.folds.retire_expired(now);
        self.drop_retired(&gone);
        if self.cfg.roster && self.cfg.fold {
            out.extend(self.roster.poll(&self.folds, now).iter().map(synthesized));
        }
        out
    }

    /// Emits folded logs whose throttle window has passed, finishes background
    /// subagents by silence, releases held entries and refreshes the roster.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        if self.guard.is_passthrough(None) {
            return Vec::new();
        }
        match catch_panic(|| self.tick_rules(now)) {
            Ok(actions) => {
                self.tick_failures = 0;
                actions
            }
            Err(payload) => {
                // Periodic work is counted apart from the line rules and never
                // switches the proxy to passthrough: it only misses a refresh.
                self.tick_failures = self.tick_failures.saturating_add(1);
                let n = self.tick_failures;
                if n <= 5 || n.is_power_of_two() {
                    vec![Action::Note(format!(
                        "fail-safe: periodic work failed ({}), {n} in a row; lines are still forwarded",
                        panic_text(payload.as_ref()),
                    ))]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Answers a prompt whose turn ended without a response: the last word
    /// was text or a usage update, nothing is open and nothing arrived for
    /// `--turn-stall-ms`. The late real response, if any, is swallowed.
    pub(super) fn watchdog(&mut self, now: Instant) -> Vec<Action> {
        let stall = self.cfg.turn_stall;
        if stall.is_zero() || self.aside {
            return Vec::new();
        }
        let mut due: Vec<String> = self
            .sessions
            .iter()
            .filter(|(sid, s)| {
                s.looks_hung(now, stall)
                    && !self.closed.contains(*sid)
                    && !self.replaying.contains_key(*sid)
                    && !self.held_steers.contains_key(*sid)
                    && !self.steers.values().any(|sp| &sp.session == *sid)
                    && !self.handoffs.values().any(|(s, _)| s == *sid)
                    && !self.guard.is_passthrough(Some(sid))
                    && self.folds.live_background(sid).is_empty()
                    && self.outstanding_prompt(sid).is_some()
            })
            .map(|(sid, _)| sid.clone())
            .collect();
        due.sort();
        let mut out = Vec::new();
        for sid in due {
            let Some(id) = self.sessions.get(&sid).and_then(|s| s.prompt_id.clone()) else {
                continue;
            };
            let quiet = self
                .sessions
                .get(&sid)
                .and_then(|s| s.last_activity)
                .map_or(0, |t| now.saturating_duration_since(t).as_millis());
            out.push(Action::Note(format!(
                "turn watchdog: prompt {id} of {sid} got no response and the agent has been silent for {quiet}ms after its final text with nothing open (--turn-stall-ms {}); answering end_turn, a late real response is dropped",
                stall.as_millis()
            )));
            let response =
                json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "end_turn"}});
            out.extend(self.on_response(&response, synthesized(&response), now));
            self.swallow.insert(id.to_string(), sid);
        }
        out
    }
}
