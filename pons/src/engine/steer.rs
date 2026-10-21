// SPDX-License-Identifier: MPL-2.0
//! Message sends and steering: a message sent during a turn is steered into it.

use super::*;

impl Engine {
    /// Handles the reply to a steering request; see the state machine at
    /// [`Engine::on_cancel`].
    pub(super) fn on_steer_reply(
        &mut self,
        sp: SteerPending,
        msg: &Value,
        now: Instant,
    ) -> Vec<Action> {
        let SteerPending {
            session: sid,
            old_key,
            new_id,
            raw,
            ended,
        } = sp;
        let outcome = msg.pointer("/result/outcome").and_then(Value::as_str);
        let mut out = Vec::new();
        // The old turn may have ended while the request was in flight.
        let finish_old = |this: &mut Self, out: &mut Vec<Action>| {
            if let Some(ended) = &ended {
                out.extend(this.on_response(ended, plain(ended), now));
            }
        };
        match outcome {
            Some("injected") => {
                out.push(Action::Note(format!(
                    "steering result: injected into the running turn of prompt {old_key}; prompt {new_id} is answered when that turn ends"
                )));
                self.folds
                    .reassign_launched_by(&sid, &old_key, &new_id.to_string());
                self.handoffs.insert(old_key, (sid, new_id));
                finish_old(self, &mut out);
            }
            Some("startedNewTurn") => {
                out.push(Action::Note(format!(
                    "steering result: startedNewTurn (the adapter runs the message as a detached turn); answering prompt {new_id} as end_turn, not forwarding it"
                )));
                finish_old(self, &mut out);
                let response =
                    json!({"jsonrpc": "2.0", "id": new_id, "result": {"stopReason": "end_turn"}});
                out.extend(self.on_response(&response, synthesized(&response), now));
            }
            _ => {
                let what = match (outcome, msg.get("error")) {
                    (Some(o), _) => o.to_owned(),
                    (None, Some(e)) => format!("error {e}"),
                    (None, None) => "no outcome".to_owned(),
                };
                out.push(Action::Note(format!(
                    "steering result: {what}; queued: forwarding prompt {new_id} to the agent"
                )));
                finish_old(self, &mut out);
                out.push(Action::ToAgent(raw));
            }
        }
        out
    }

    /// Releases the steering holds whose time is up.
    pub(super) fn release_steers_due(&mut self, now: Instant) -> Vec<Action> {
        let mut due: Vec<String> = self
            .held_steers
            .iter()
            .filter(|(_, hs)| now >= hs.until)
            .map(|(sid, _)| sid.clone())
            .collect();
        due.sort();
        let mut out = Vec::new();
        for sid in due {
            out.extend(self.flush_held_steer(&sid, now, "hold time passed, steering anyway"));
        }
        out
    }

    /// Sends the steering that waited for a tool call, if the session has one.
    pub(super) fn flush_held_steer(&mut self, sid: &str, _now: Instant, why: &str) -> Vec<Action> {
        let Some(hs) = self.held_steers.remove(sid) else {
            return Vec::new();
        };
        let head = format!("{}; hold over ({why})", hs.head);
        self.start_steer(sid, &hs.id, &hs.prompt, &hs.raw, hs.old_key, &head)
    }

    /// Sends `_session/steering` for the message `id` in place of its prompt.
    pub(super) fn start_steer(
        &mut self,
        sid: &str,
        id: &Value,
        prompt: &Value,
        raw: &[u8],
        old_key: String,
        head: &str,
    ) -> Vec<Action> {
        self.steer_seq += 1;
        let req_id = format!("pons-steer-{}", self.steer_seq);
        let request = json!({"jsonrpc": "2.0", "id": req_id, "method": "_session/steering",
            "params": {"sessionId": sid, "prompt": prompt,
                "_meta": {"steering": {"idleBehavior": "promptRequired"}}}});
        self.steers.insert(
            json!(req_id).to_string(),
            SteerPending {
                session: sid.to_owned(),
                old_key,
                new_id: id.clone(),
                raw: raw.to_vec(),
                ended: None,
            },
        );
        vec![
            Action::Note(format!(
                "{head}; steered: sent {req_id} (_session/steering) in place of prompt {id}"
            )),
            Action::ToAgent(request.to_string().into_bytes()),
        ]
    }

    /// Handles a prompt that arrived while a cancel was held: the user sent a message, so
    /// the cancel is dropped and the subagents live on. The prompt is steered
    /// into the running turn, or forwarded to queue behind it.
    pub(super) fn on_message_send(
        &mut self,
        sid: &str,
        msg: &Value,
        raw: &[u8],
        held: &PendingCancel,
        now: Instant,
    ) -> Vec<Action> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let waited = now.saturating_duration_since(held.at).as_millis();
        let prompt = msg
            .pointer("/params/prompt")
            .filter(|p| p.as_array().is_some_and(|a| !a.is_empty()));
        let want_steer = self.cfg.interrupt != Some(InterruptMode::Queue);
        let head = format!(
            "message sent while subagents run: dropped the held session/cancel for {sid} ({waited}ms before prompt {id})"
        );
        let (Some(prompt), true) = (prompt, want_steer && self.steering) else {
            let why = if !want_steer {
                "--interrupt-mode queue"
            } else {
                "the agent did not advertise steering"
            };
            return vec![
                Action::Note(format!(
                    "{head}; queued: forwarding prompt {id} to the agent ({why})"
                )),
                Action::ToAgent(raw.to_vec()),
            ];
        };
        let old_key = held.prompt.to_string();
        let hold = self.cfg.steer_hold;
        if !hold.is_zero()
            && self
                .sessions
                .get(sid)
                .is_some_and(Session::has_streaming_tool)
        {
            // A tool call still streams its input: steering now can race it
            // (the adapter's incomplete_tool_call error). Wait for it.
            let note = format!(
                "{head}; holding the steering of prompt {id} for up to {}ms: a tool call is still streaming its input",
                hold.as_millis()
            );
            self.held_steers.insert(
                sid.to_owned(),
                HeldSteer {
                    id,
                    raw: raw.to_vec(),
                    prompt: prompt.clone(),
                    old_key,
                    until: now + hold,
                    head,
                },
            );
            return vec![Action::Note(note)];
        }
        self.start_steer(sid, &id, prompt, raw, old_key, &head)
    }
}
