// SPDX-License-Identifier: MPL-2.0

//! Decides what happens to each line of each direction.

use super::*;

impl Engine {
    /// Drops everything kept for `sid` (the client closed it): no more
    /// updates or ticks come out for it.
    fn forget_session(&mut self, sid: &str) {
        self.folds.forget_session(sid);
        self.roster.forget(sid);
        self.guard.forget(sid);
        self.notes.forget(sid);
        self.sessions.remove(sid);
        self.prompts.retain(|_, s| s != sid);
        self.swallow.retain(|_, s| s != sid);
        self.steers.retain(|_, sp| sp.session != sid);
        self.held_steers.remove(sid);
        self.handoffs.retain(|_, (s, _)| s != sid);
        self.held.retain(|h| h.session != sid);
        self.replay_dropped.remove(sid);
        self.replay_markers.remove(sid);
        self.replaying.remove(sid);
        self.loads.retain(|_, s| s != sid);
        self.closed.insert(sid.to_owned());
    }

    /// Handles a line from the client. A failing rule never loses the line:
    /// it is forwarded as it came (see [`Engine::guarded`]).
    pub fn on_client(&mut self, raw: &[u8], now: Instant) -> Vec<Action> {
        let forward = |raw: &[u8]| Action::ToAgent(raw.to_vec());
        self.guarded(raw, now, forward, Self::client_rules)
    }

    /// Handles a line from the agent; same guarantee as [`Engine::on_client`].
    pub fn on_agent(&mut self, raw: &[u8], now: Instant) -> Vec<Action> {
        self.summary = None;
        let forward = |raw: &[u8]| Action::ToClient {
            line: raw.to_vec(),
            synthesized: false,
        };
        if self.guard.any_passthrough()
            && let Some(dropped) = self.passthrough_swallow(raw)
        {
            return dropped;
        }
        self.guarded(raw, now, forward, Self::agent_rules)
    }

    /// Swallows the response to a prompt that pons already answered: in passthrough
    /// the rules are off, but this response must still not reach the client a second time.
    fn passthrough_swallow(&mut self, raw: &[u8]) -> Option<Vec<Action>> {
        if self.swallow.is_empty() && self.steers.is_empty() {
            return None;
        }
        let msg: Value = serde_json::from_slice(raw).ok()?;
        if msg.get("result").is_none() && msg.get("error").is_none() {
            return None;
        }
        let key = msg.get("id")?.to_string();
        if let Some(sp) = self.steers.get(&key)
            && self.guard.is_passthrough(Some(&sp.session))
        {
            self.steers.remove(&key);
            return Some(vec![Action::Note(format!(
                "swallowed steering reply {key} (passthrough)"
            ))]);
        }
        let sid = self.swallow.get(&key)?.clone();
        if !self.guard.is_passthrough(None) && !self.guard.is_passthrough(Some(&sid)) {
            return None;
        }
        self.swallow.remove(&key);
        Some(vec![Action::Note(format!(
            "swallowed agent response for cancelled prompt {key} (passthrough)"
        ))])
    }

    /// Runs `rules` on one line. If they panic (the release-unwind profile unwinds
    /// for this), the original line is forwarded unchanged and the failure
    /// is noted; after `max_failures` in a row the session is passthrough.
    /// Engine state can be half-updated after a panic; that is acceptable
    /// because the only consequence is a missed rewrite, never a lost line.
    fn guarded(
        &mut self,
        raw: &[u8],
        now: Instant,
        forward: fn(&[u8]) -> Action,
        rules: fn(&mut Self, &[u8], Instant) -> Vec<Action>,
    ) -> Vec<Action> {
        if self.guard.any_passthrough() && self.guard.is_passthrough(quick_session(raw).as_deref())
        {
            return vec![forward(raw)];
        }
        let outcome = catch_panic(|| {
            if let Some(pattern) = &self.cfg.inject_failure
                && contains(raw, pattern.as_bytes())
            {
                panic!("injected failure ({pattern})");
            }
            rules(self, raw, now)
        });
        match outcome {
            Ok(actions) => {
                if self.guard.has_failures() {
                    self.guard.success(quick_session(raw).as_deref());
                }
                actions
            }
            Err(payload) => {
                let sid = quick_session(raw);
                let failure = self.guard.failure(sid.as_deref());
                let scope = sid.as_deref().unwrap_or("the proxy");
                let mut note = format!(
                    "fail-safe: rules failed ({}), forwarded the line unchanged ({} in a row for {scope})",
                    panic_text(payload.as_ref()),
                    failure.count
                );
                if failure.switched {
                    note.push_str("; switching to pure passthrough");
                }
                vec![Action::Note(note), forward(raw)]
            }
        }
    }

    /// Runs the bookkeeping that precedes the rules: a steering that waits for a tool
    /// call is sent before a cancel or a newer prompt of its session is
    /// handled, and a client response ends the agent request it answers.
    fn client_rules(&mut self, raw: &[u8], now: Instant) -> Vec<Action> {
        let Ok(msg) = serde_json::from_slice::<Value>(raw) else {
            return vec![
                Action::Note("malformed client line, forwarded raw".into()),
                Action::ToAgent(raw.to_vec()),
            ];
        };
        let mut out = Vec::new();
        match method(&msg) {
            Some("session/cancel" | "session/prompt") => {
                if let Some(sid) = session_id(&msg) {
                    out = self.flush_held_steer(sid, now, "another message of the session arrived");
                }
            }
            None if msg.get("result").is_some() || msg.get("error").is_some() => {
                if let Some(id) = msg.get("id") {
                    let key = id.to_string();
                    for s in self.sessions.values_mut() {
                        if s.asking.remove(&key) {
                            s.last_activity = Some(now);
                        }
                    }
                }
            }
            _ => {}
        }
        out.extend(self.route_client(msg, raw, now));
        out
    }

    fn route_client(&mut self, msg: Value, raw: &[u8], now: Instant) -> Vec<Action> {
        let forward = || vec![Action::ToAgent(raw.to_vec())];
        match method(&msg) {
            Some("initialize") => self.on_initialize(msg, raw),
            Some("session/new") => self.add_mcp_servers(msg, raw),
            Some("session/load" | "session/resume") => {
                if let (Some(id), Some(sid)) = (msg.get("id"), session_id(&msg)) {
                    self.closed.remove(sid);
                    self.loads.insert(id.to_string(), sid.to_owned());
                    self.replaying.insert(sid.to_owned(), 0);
                    self.replay_dropped.remove(sid);
                    self.replay_markers.remove(sid);
                }
                self.add_mcp_servers(msg, raw)
            }
            Some("session/prompt") => {
                if let (Some(id), Some(sid)) = (msg.get("id"), session_id(&msg)) {
                    self.closed.remove(sid);
                    self.replaying.remove(sid);
                    self.prompts.insert(id.to_string(), sid.to_owned());
                    let s = self.sessions.entry(sid.to_owned()).or_default();
                    let held = s.pending_cancel.take();
                    s.prompt_id = Some(id.clone());
                    s.dropped_cancel = None;
                    s.last_main = Some(now);
                    s.last_activity = Some(now);
                    s.final_text = false;
                    s.turn_started = Some(now);
                    self.roster.on_prompt(sid);
                    if let Some(held) = held {
                        let mut out = self.on_message_send(sid, &msg, raw, &held, now);
                        out.extend(self.reassert_cards(sid, now));
                        return out;
                    }
                }
                forward()
            }
            Some("session/close") => {
                let mut out = Vec::new();
                if let Some(sid) = session_id(&msg) {
                    out.push(Action::Note(format!(
                        "session {sid} closed by the client: dropped its state"
                    )));
                    self.forget_session(sid);
                }
                out.extend(forward());
                out
            }
            Some("session/cancel") if self.cfg.cancel_filter && !self.aside => self
                .on_cancel(&msg, raw, now)
                .unwrap_or_else(|| self.on_cancel_forwarded(&msg, raw, now)),
            _ => forward(),
        }
    }

    /// Handles the bookkeeping for a request from the agent. `Err` is the complete answer
    /// (pons answered a permission request itself); `Ok` holds log lines for a
    /// request that is forwarded as it came.
    fn agent_request(&mut self, msg: &Value, now: Instant) -> Result<Vec<Action>, Vec<Action>> {
        let (Some(m), Some(id)) = (method(msg), msg.get("id")) else {
            return Ok(Vec::new());
        };
        let sid = session_id(msg).filter(|s| !self.closed.contains(*s));
        if let Some(sid) = sid {
            let s = self.sessions.entry(sid.to_owned()).or_default();
            s.last_activity = Some(now);
        }
        let mut notes = Vec::new();
        if m == "session/request_permission" && self.policy.is_on() {
            let params = msg.get("params").unwrap_or(&Value::Null);
            let tool = permission_tool_call_id(params);
            let tracked = sid
                .zip(tool)
                .and_then(|(s, t)| self.sessions.get(s)?.open_tools.get(t));
            let known = tracked.map_or_else(Known::default, |t| Known {
                title: &t.title,
                kind: &t.kind,
                name: &t.name,
            });
            match self.policy.decide(params, known) {
                Decision::Answer { option_id, why } => {
                    let reply = json!({"jsonrpc": "2.0", "id": id,
                    "result": {"outcome": {"outcome": "selected", "optionId": option_id}}});
                    return Err(vec![
                        Action::Note(format!(
                            "auto-approved permission request {id} (tool call {}): {why}; the client was not asked",
                            tool.unwrap_or("?")
                        )),
                        Action::ToAgent(reply.to_string().into_bytes()),
                    ]);
                }
                Decision::Forward(why) => notes.push(Action::Note(format!(
                    "permission request {id} forwarded to the client: {why}"
                ))),
            }
        }
        if let Some(sid) = sid
            && let Some(s) = self.sessions.get_mut(sid)
        {
            s.asking.insert(id.to_string());
        }
        Ok(notes)
    }

    fn agent_rules(&mut self, raw: &[u8], now: Instant) -> Vec<Action> {
        let forward = || Action::ToClient {
            line: raw.to_vec(),
            synthesized: false,
        };
        let Ok(msg) = serde_json::from_slice::<Value>(raw) else {
            return vec![
                Action::Note("malformed agent line, forwarded raw".into()),
                forward(),
            ];
        };
        if self.want_summary {
            self.summary = crate::log::summarize_message(&msg);
        }
        let mut out = match self.agent_request(&msg, now) {
            Ok(notes) => notes,
            Err(done) => return done,
        };
        if let Some(file) = &self.cfg.census {
            let t = crate::census::unix_seconds();
            out.extend(
                self.limits
                    .observe(&msg, t)
                    .into_iter()
                    .map(|snapshot| Action::Census {
                        file: file.clone(),
                        snapshot,
                    }),
            );
        }
        if self.cfg.drop_task_notifications || self.cfg.drop_interrupt_markers {
            match self.filter_notifications(&msg, raw, now) {
                Ok(before) => out.extend(before),
                Err(done) => return done,
            }
        }
        out.extend(match method(&msg) {
            Some("session/update") => self.on_update(&msg, forward(), now),
            Some(_) => vec![forward()],
            None => self.on_response(&msg, forward(), now),
        });
        out.extend(self.fold_notes());
        out
    }
}
