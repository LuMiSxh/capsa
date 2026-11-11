// SPDX-License-Identifier: MPL-2.0
//! Handles prompt responses and turn ends.

use super::*;

impl Engine {
    /// Forgets the finished subagents' in-flight state, emits their cards (and
    /// notices) and holds their entries for a safe boundary.
    pub(super) fn finished(&mut self, finished: Vec<Finished>) -> Vec<Action> {
        let notices = match self.cfg.notices {
            NoticeMode::Always => true,
            NoticeMode::Auto => self.client_notices,
            NoticeMode::Never => false,
        };
        let mut out = Vec::new();
        for f in finished {
            if f.confirmed
                && let Some(s) = self.sessions.get_mut(&f.session)
            {
                s.in_flight.remove(&f.parent);
            }
            out.push(Action::Note(format!(
                "background subagent {} finished ({}{})",
                f.parent,
                f.reason,
                if f.confirmed { "" } else { ", provisional" }
            )));
            if let Some(update) = &f.update {
                out.push(synthesized(update));
            }
            if notices && f.confirmed {
                out.push(synthesized(&f.notice));
            }
            match f.entry {
                Some(EntryOut::Append(update)) => self.held.push(Held {
                    session: f.session,
                    parent: f.parent,
                    update,
                }),
                Some(EntryOut::Update(update)) => out.push(synthesized(&update)),
                None => {}
            }
        }
        out
    }

    /// Marks the running background subagents of `sid` as stopped after a real Stop.
    pub(super) fn finish_all(&mut self, sid: &str, now: Instant) -> Vec<Action> {
        let finished = self.folds.stop_session(sid, now);
        self.finished(finished)
    }

    /// Ends the turn after the adapter answered its prompt: flushes folds, sends the
    /// held entries before the response, and forgets turn-scoped state.
    /// `completed` is true for a response the client sees, which also retires
    /// finished subagents that were already done when the turn began.
    pub(super) fn end_turn(&mut self, sid: &str, now: Instant, completed: bool) -> Vec<Action> {
        let mut out: Vec<Action> = self
            .folds
            .flush_session(sid, now)
            .iter()
            .map(synthesized)
            .collect();
        out.extend(self.release_session(sid, "turn end"));
        if completed {
            let started = self.sessions.get(sid).and_then(|s| s.turn_started);
            let gone = self.folds.retire(sid, started);
            self.drop_retired(&gone);
        }
        self.folds.clear_session(sid);
        // Cards finished only by silence may still be working: they keep
        // holding the cancel filter.
        let live = self.folds.live_background(sid);
        if let Some(s) = self.sessions.get_mut(sid) {
            s.end_turn(&live);
        }
        out
    }

    /// Forgets the in-flight state of retired cards.
    pub(super) fn drop_retired(&mut self, gone: &[(String, String)]) {
        for (sid, parent) in gone {
            if let Some(s) = self.sessions.get_mut(sid) {
                s.in_flight.remove(parent);
                s.parents.remove(parent);
            }
        }
    }

    /// Finishes the cards that prompt `key` launched: the adapter holds a prompt open
    /// until every subagent launched in its turn is gone, so those cards are done.
    pub(super) fn finish_launched(
        &mut self,
        sid: &str,
        key: &str,
        failed: bool,
        now: Instant,
    ) -> Vec<Action> {
        let finished = self.folds.finish_launched_by(sid, key, failed, now);
        if finished.is_empty() {
            return Vec::new();
        }
        let mut out = vec![Action::Note(format!(
            "prompt {key} answered: finishing {} background subagent(s) it launched",
            finished.len()
        ))];
        out.extend(self.finished(finished));
        out
    }

    /// Returns the newest prompt of `sid` that has no response yet, as JSON text.
    pub(super) fn outstanding_prompt(&self, sid: &str) -> Option<String> {
        self.sessions
            .get(sid)?
            .prompt_id
            .as_ref()
            .map(Value::to_string)
            .filter(|key| self.prompts.contains_key(key))
    }

    pub(super) fn on_response(
        &mut self,
        msg: &Value,
        forward: Action,
        now: Instant,
    ) -> Vec<Action> {
        let view = ResponseView::new(msg);
        let Some(id) = view.response_id() else {
            return vec![forward];
        };
        let key = id.to_string();
        let mut out = Vec::new();
        if self.init_id.as_deref() == Some(key.as_str()) {
            self.init_id = None;
            if view.steering_supported {
                self.steering = true;
                out.push(Action::Note(
                    "the agent advertised steering (_meta.steering.supported)".into(),
                ));
            }
        }
        if let Some(sp) = self.steers.remove(&key) {
            out.extend(self.on_steer_reply(sp, msg, now));
            return out;
        }
        if let Some(sid) = self.loads.remove(&key)
            && let Some(skipped) = self.replaying.remove(&sid)
        {
            let dropped = self.replay_dropped.remove(&sid).unwrap_or(0);
            let markers = self.replay_markers.remove(&sid).unwrap_or(0);
            let mut tail = String::new();
            if dropped > 0 {
                tail.push_str(&format!(
                    "; dropped {dropped} replayed subagent text chunk(s)"
                ));
            }
            if markers > 0 {
                tail.push_str(&format!("; dropped {markers} replayed interrupt marker(s)"));
            }
            out.push(Action::Note(format!(
                "replay of {sid} done, {skipped} update(s) seen, no subagent state kept{tail}"
            )));
        }
        let cancelled = view.cancelled;
        if let Some(sp) = self.steers.values_mut().find(|sp| sp.old_key == key) {
            sp.ended = Some(msg.clone());
            out.push(Action::Note(format!(
                "response to prompt {id} arrived before the steering reply; held until it comes"
            )));
            return out;
        }
        if let Some(sid) = self.swallow.remove(&key) {
            out.extend(self.on_swallowed(&sid, &key, id, msg, cancelled, now));
            return out;
        }
        if let Some(sid) = self.prompts.remove(&key) {
            out.extend(self.on_prompt_response(&sid, &key, id, cancelled, forward, now));
            return out;
        }
        out.push(forward);
        out
    }

    /// Answers a prompt pons tracks: a handoff when a newer prompt is
    /// outstanding, else the end of its turn.
    pub(super) fn on_prompt_response(
        &mut self,
        sid: &str,
        key: &str,
        id: &Value,
        cancelled: bool,
        forward: Action,
        now: Instant,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(s) = self.sessions.get_mut(sid)
            && s.prompt_id.as_ref() == Some(id)
        {
            s.prompt_id = None;
        }
        if let Some(newer) = self.outstanding_prompt(sid) {
            // A handoff: the adapter answers the old prompt because a
            // newer one took over. The cards belong to the newer one.
            self.folds.reassign_launched_by(sid, key, &newer);
            out.push(Action::Note(format!(
                "prompt {id} answered while prompt {newer} is outstanding: handoff, background subagents stay"
            )));
            if cancelled {
                out.extend(self.close_ghosts(sid, Some(key), "handoff after a cancelled prompt"));
            }
            out.extend(self.folds.flush_session(sid, now).iter().map(synthesized));
            out.extend(self.release_session(sid, "prompt handoff"));
        } else {
            if cancelled {
                out.extend(self.close_ghosts(sid, None, "prompt answered as cancelled"));
            }
            out.extend(self.finish_launched(sid, key, cancelled, now));
            out.extend(self.end_turn(sid, now, true));
        }
        out.push(forward);
        out
    }
}
