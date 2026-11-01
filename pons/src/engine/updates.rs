// SPDX-License-Identifier: MPL-2.0
//! Session updates from the agent: subagent folding and the task-notification filter.

use super::*;

impl Engine {
    /// Handles the parent's own update, which is not main-agent activity.
    pub(super) fn on_parent_update(
        &mut self,
        sid: &str,
        msg: &Value,
        forward: Action,
        now: Instant,
    ) -> Vec<Action> {
        let update = msg.pointer("/params/update").unwrap_or(&Value::Null);
        let kind = update.get("sessionUpdate").and_then(Value::as_str);
        let tool_id = update.get("toolCallId").and_then(Value::as_str);
        let id = tool_id.unwrap_or_default();
        self.folds.note_input(id, update);
        let session = self.sessions.entry(sid.to_owned()).or_default();
        let terminal = matches!(
            update.get("status").and_then(Value::as_str),
            Some("completed" | "failed")
        );
        if self.cfg.fold
            && kind == Some("tool_call_update")
            && update.get("status").and_then(Value::as_str) == Some("completed")
            && is_async_launch(update)
        {
            // The adapter reports a background launch as completed; the
            // subagent still runs, so the card stays open.
            session.in_flight.insert(id.to_owned());
            let mut shown = msg.clone();
            let by = session.prompt_id.as_ref().map(Value::to_string);
            shown["params"]["update"] =
                self.folds
                    .launch_background(sid, id, update, by.as_deref(), now);
            return vec![
                Action::Note(format!("background subagent {id} launched, card kept open")),
                synthesized(&shown),
            ];
        }
        if terminal {
            session.in_flight.remove(id);
        }
        let mut out = Vec::new();
        if self.cfg.fold
            && kind == Some("tool_call_update")
            && let Some(flushed) = self.folds.parent_update(id, update, terminal, now)
        {
            out.push(synthesized(&flushed));
        }
        if kind == Some("tool_call") {
            out.extend(self.release_session(sid, "main agent tool call"));
        }
        out.push(forward);
        out
    }

    /// Folds a child of a subagent parent into the parent's card.
    pub(super) fn on_child_update(
        &mut self,
        sid: &str,
        parent: &str,
        update: &Value,
        forward: Action,
        now: Instant,
    ) -> Vec<Action> {
        let root = self.folds.resolve(parent);
        let session = self.sessions.entry(sid.to_owned()).or_default();
        // A tool call that children point at is a parent, whatever its name.
        let learned = session.parents.insert(root.clone());
        session.in_flight.insert(root.clone());
        if !self.cfg.fold {
            return vec![forward];
        }
        let mut out = Vec::new();
        if learned {
            out.push(Action::Note(format!(
                "tool call {root} became a subagent parent through parentToolUseId"
            )));
        }
        out.extend(
            self.folds
                .child_update(sid, &root, update, now)
                .iter()
                .map(synthesized),
        );
        if update.get("sessionUpdate").and_then(Value::as_str) == Some("tool_call") {
            out.push(Action::Note(child_start_note(&root, update)));
        }
        // Activity after a finish: the entry announcing it was premature.
        let folds = &self.folds;
        self.held
            .retain(|h| h.parent != root || folds.is_finished(&root));
        out
    }

    pub(super) fn route_update(
        &mut self,
        msg: &Value,
        forward: Action,
        now: Instant,
    ) -> Vec<Action> {
        let Some(sid) = session_id(msg) else {
            return vec![forward];
        };
        let update = msg.pointer("/params/update").unwrap_or(&Value::Null);
        let session = self.sessions.entry(sid.to_owned()).or_default();
        session.last_activity = Some(now);
        if let Some(parent) = parent_tool_use_id(msg, update) {
            return self.on_child_update(sid, parent, update, forward, now);
        }

        let tool_id = update.get("toolCallId").and_then(Value::as_str);
        let kind = update.get("sessionUpdate").and_then(Value::as_str);
        if kind == Some("tool_call")
            && let Some(id) = tool_id
            && is_subagent_tool(update)
        {
            session.parents.insert(id.to_owned());
            self.folds.register_parent(sid, update);
        }
        let is_parent = tool_id.is_some_and(|id| session.parents.contains(id));
        if is_parent {
            return self.on_parent_update(sid, msg, forward, now);
        }
        session.last_main = Some(now);
        match kind {
            Some("agent_message_chunk" | "usage_update") => session.final_text = true,
            Some("tool_call" | "tool_call_update" | "agent_thought_chunk") => {
                session.final_text = false;
            }
            _ => {}
        }
        if let Some(id) = tool_id {
            track_tool(session, id, kind, update);
        }
        if matches!(kind, Some("agent_message_chunk" | "agent_thought_chunk")) {
            session.last_text = Some(now);
        }
        let mut out = Vec::new();
        if kind == Some("tool_call") {
            // A tool call starts a new block: a safe place for entries.
            out.extend(self.release_session(sid, "main agent tool call"));
        }
        if kind == Some("plan") && self.cfg.roster && self.cfg.fold {
            let items = self.folds.roster(sid, now);
            if let Some(merged) = self.roster.on_adapter_plan(sid, update, &items) {
                out.push(synthesized(&merged));
                return out;
            }
        }
        out.push(forward);
        out
    }

    pub(super) fn on_update(&mut self, msg: &Value, forward: Action, now: Instant) -> Vec<Action> {
        if self.aside || session_id(msg).is_some_and(|s| self.closed.contains(s)) {
            return vec![forward];
        }
        if let Some(sid) = session_id(msg)
            && let Some(skipped) = self.replaying.get_mut(sid)
        {
            // History being replayed: nothing in it is running, so it must
            // not register parents or cards that pons would later "finish".
            *skipped += 1;
            // With subagent text on, the adapter replays child text too; it
            // would show flat in the chat, and the cards of old subagents
            // are not rebuilt, so it is dropped.
            let update = msg.pointer("/params/update").unwrap_or(&Value::Null);
            if self.forwards_text
                && parent_tool_use_id(msg, update).is_some()
                && matches!(
                    update.get("sessionUpdate").and_then(Value::as_str),
                    Some("agent_message_chunk" | "agent_thought_chunk")
                )
            {
                *self.replay_dropped.entry(sid.to_owned()).or_default() += 1;
                return Vec::new();
            }
            return vec![forward];
        }
        let mut out = Vec::new();
        if self.cfg.fold
            && let Some(sid) = session_id(msg)
        {
            let update = msg.pointer("/params/update").unwrap_or(&Value::Null);
            if parent_tool_use_id(msg, update).is_none() && is_task_notification(update) {
                // The adapter ends the turn that handled the notification with
                // this marker, so its text is complete: finish the subagent
                // now and let the held entries out.
                self.folds.note_notification(sid, now);
                let finished = self.folds.poll(now);
                out.extend(self.finished(finished));
                out.extend(self.release_session(sid, "task-notification"));
            }
        }
        out.extend(self.route_update(msg, forward, now));
        if let Some(sid) = session_id(msg)
            && self.held_steers.contains_key(sid)
            && !self
                .sessions
                .get(sid)
                .is_some_and(Session::has_streaming_tool)
        {
            out.extend(self.flush_held_steer(sid, now, "the tool call's input is complete"));
        }
        out
    }

    /// Handles a response to a prompt whose turn pons swallowed (it was cancelled).
    pub(super) fn on_swallowed(
        &mut self,
        sid: &str,
        key: &str,
        id: &Value,
        msg: &Value,
        cancelled: bool,
        now: Instant,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        if self
            .held_steers
            .get(sid)
            .is_some_and(|hs| hs.old_key == key)
            && let Some(hs) = self.held_steers.remove(sid)
        {
            out.push(Action::Note(format!(
                "the turn of prompt {id} ended while the steering of prompt {} waited for a tool call; queued: forwarding the prompt",
                hs.id
            )));
            out.push(Action::ToAgent(hs.raw));
        }
        out.push(Action::Note(format!(
            "swallowed agent response for cancelled prompt {id}"
        )));
        if let Some((_, new_id)) = self.handoffs.remove(key) {
            out.push(Action::Note(format!(
                "the turn of prompt {id} ended: answering steered prompt {new_id} with its result"
            )));
            let mut response = msg.clone();
            response["id"] = new_id;
            out.extend(self.on_response(&response, synthesized(&response), now));
            return out;
        }
        if let Some(s) = self.sessions.get_mut(sid) {
            // The turn is over: a held cancel has nothing left to stop.
            s.pending_cancel = None;
        }
        if let Some(newer) = self.outstanding_prompt(sid) {
            // A newer turn is running: nothing of this response is its.
            self.folds.reassign_launched_by(sid, key, &newer);
            out.push(Action::Note(format!(
                "the swallowed response belongs to an earlier turn of {sid}; prompt {newer} is outstanding, its state is left alone"
            )));
            return out;
        }
        out.extend(self.finish_launched(sid, key, cancelled, now));
        out.extend(self.end_turn(sid, now, false));
        out
    }

    /// Runs the task-notification filter over an agent message. `Ok` holds
    /// the lines to send before the message, which is then handled as usual;
    /// `Err` is the complete answer for it.
    pub(super) fn filter_notifications(
        &mut self,
        msg: &Value,
        raw: &[u8],
        now: Instant,
    ) -> Result<Vec<Action>, Vec<Action>> {
        let sid = session_id(msg);
        let watch = Watch {
            notifications: self.cfg.drop_task_notifications,
            markers: self.cfg.drop_interrupt_markers
                && sid.is_some_and(|s| self.replaying.contains_key(s)),
        };
        let verdict = match (method(msg), sid) {
            (Some("session/update"), Some(sid)) if !self.closed.contains(sid) => {
                let update = msg.pointer("/params/update").unwrap_or(&Value::Null);
                self.notes.on_update(sid, update, raw, now, watch)
            }
            _ => Verdict::Pass,
        };
        let plain = |lines: Vec<Vec<u8>>| {
            lines
                .into_iter()
                .map(|line| Action::ToClient {
                    line,
                    synthesized: false,
                })
                .collect::<Vec<_>>()
        };
        match verdict {
            Verdict::Pass if self.notes.is_empty() => Ok(Vec::new()),
            Verdict::Pass => Ok(plain(self.notes.flush(sid))),
            Verdict::Hold => Err(Vec::new()),
            Verdict::Release(lines) => Err(plain(lines)),
            Verdict::DropMarker => {
                if let Some(sid) = sid {
                    *self.replay_markers.entry(sid.to_owned()).or_default() += 1;
                }
                Err(Vec::new())
            }
            Verdict::Drop { chars, rest } => {
                let mut out = vec![Action::Note(format!(
                    "dropped a task-notification user message ({chars} chars)"
                ))];
                if let Some(update) = rest {
                    let mut shown = msg.clone();
                    shown["params"]["update"] = update;
                    out.push(synthesized(&shown));
                }
                Err(out)
            }
        }
    }

    /// Returns the folds' decisions worth a log line.
    pub(super) fn fold_notes(&mut self) -> Vec<Action> {
        self.folds
            .take_notes()
            .into_iter()
            .map(Action::Note)
            .collect()
    }
}
