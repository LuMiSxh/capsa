// SPDX-License-Identifier: MPL-2.0
//! One tool-call entry of a folded subagent: its text, status and finished notice.

use super::label::subagent_label;
use super::*;

impl FoldEntry {
    pub(super) fn calls(&self) -> String {
        match self.tool_calls {
            1 => "1 tool call".to_owned(),
            n => format!("{n} tool calls"),
        }
    }

    /// Ends the card with a final log line and returns the update to send.
    pub(super) fn finish(
        &mut self,
        failed: bool,
        by: Option<Instant>,
        footer: String,
        now: Instant,
    ) -> Value {
        self.done = Some(Done { failed, by });
        self.finished_at = Some(now);
        self.footer = Some(footer);
        self.dirty = true;
        self.last_emit = None;
        self.take_emit(now).unwrap_or(Value::Null)
    }

    /// Tells whether the `task-notification` at `at` can have been this subagent's. It must
    /// not be working (a tool open) and must not have spoken since.
    pub(super) fn marker_candidate(&self, at: Instant) -> bool {
        self.background
            && (self.is_running_background() || self.is_provisional())
            && self.open_tools.is_empty()
            && self.last_child.is_none_or(|c| c <= at)
    }

    /// Returns why the silence rule would finish this card now, if it would.
    /// Only the subagent's own text counts: it thinks or runs a long tool
    /// silently, so any other last event, and a tool result in particular,
    /// cannot tell a finished subagent from a running one.
    pub(super) fn silent_finish(
        &self,
        now: Instant,
        idle: Duration,
        cap: Duration,
        text_visible: bool,
    ) -> Option<&'static str> {
        if !self.is_running_background() || !self.open_tools.is_empty() || self.monitoring(now) {
            return None;
        }
        let silent = self.silence(now);
        if text_visible {
            if self.last_kind != Kind::Text || idle.is_zero() {
                return None;
            }
            (silent >= idle).then_some("idle")
        } else {
            (!cap.is_zero() && silent >= cap).then_some("quiet-cap")
        }
    }

    /// Tracks the subagent's `Monitor` calls, which keep streaming after the call itself
    /// completed. A later update of the call may carry the timeout its input lacked.
    pub(super) fn note_monitor<'a>(&mut self, update: impl Into<UpdateView<'a>>, now: Instant) {
        let update = update.into();
        let Some(id) = update.tool_call_id else {
            return;
        };
        if update.session_update == Some("tool_call") && update.is_monitor() {
            self.monitors.retain(|_, m| m.deadline() > now);
            self.monitors.entry(id.to_owned()).or_insert(Monitor {
                started: now,
                timeout: None,
            });
        }
        if let Some(timeout) = update.monitor_timeout()
            && let Some(monitor) = self.monitors.get_mut(id)
        {
            monitor.timeout = Some(timeout);
        }
    }

    /// Tells whether a `Monitor` call of the subagent may still be streaming events.
    pub(super) fn monitoring(&self, now: Instant) -> bool {
        self.monitors.values().any(|m| now < m.deadline())
    }

    /// Tells whether the card is finished by silence only, and not yet confirmed or hidden.
    pub(super) fn is_provisional(&self) -> bool {
        self.background && self.done.is_some() && self.provisional
    }

    pub(super) fn is_running_background(&self) -> bool {
        self.background && self.done.is_none()
    }

    pub(super) fn last_activity(&self) -> Option<Instant> {
        self.last_child.or(self.launched)
    }

    /// Returns the card status pons asserts: only background cards, whose status the
    /// adapter set to `completed` long before the subagent ended.
    pub(super) fn status(&self) -> Option<&'static str> {
        self.background.then_some(match self.done {
            None => "in_progress",
            Some(Done { failed: true, .. }) => "failed",
            Some(_) => "completed",
        })
    }

    /// Returns the update to send: the full content when the log changed, only the
    /// status and title when just the clock moved.
    pub(super) fn take_emit(&mut self, now: Instant) -> Option<Value> {
        if !self.dirty && !self.title_dirty {
            return None;
        }
        let full = self.dirty;
        self.dirty = false;
        self.title_dirty = false;
        self.last_emit = Some(now);
        let mut msg = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": self.session,
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": self.parent,
                },
            },
        });
        if full {
            msg["params"]["update"]["content"] = json!(self.content());
        }
        if let Some(status) = self.status() {
            msg["params"]["update"]["status"] = json!(status);
        }
        if let Some(title) = self.card_title(now) {
            msg["params"]["update"]["title"] = json!(title);
            self.last_title = Some(title);
        }
        Some(msg)
    }

    pub(super) fn is_due(&self, now: Instant, throttle: Duration) -> bool {
        (self.dirty || self.title_dirty)
            && self
                .last_emit
                .is_none_or(|t| now.saturating_duration_since(t) >= throttle)
    }

    /// Updates `open_tools` and `last_kind`. A tool call starts open
    /// unless it already carries an ended status; an update without a status
    /// changes neither.
    pub(super) fn track_tool(&mut self, id: Option<&str>, is_call: bool, state: Option<LineState>) {
        let ended = matches!(state, Some(LineState::Done | LineState::Failed));
        if ended {
            if let Some(id) = id {
                self.open_tools.remove(id);
            }
            self.last_kind = Kind::ToolResult;
        } else if is_call || state == Some(LineState::Running) {
            if let Some(id) = id
                && self.open_tools.len() < MAX_OPEN_TOOLS
            {
                self.open_tools.insert(id.to_owned());
            }
            self.last_kind = Kind::ToolOpen;
        }
    }

    pub(super) fn apply_tool(&mut self, update: UpdateView<'_>, is_call: bool) {
        let id = update.tool_call_id;
        let status = update.status;
        let title = update
            .title
            .map(one_line)
            .filter(|t| !t.is_empty())
            .map(|t| clip_head(&t, MAX_TITLE_CHARS));
        let state = status.map(LineState::from_status);
        self.track_tool(id, is_call, state);
        let existing =
            id.and_then(|id| self.lines.iter_mut().find(|l| l.key.as_deref() == Some(id)));
        match (existing, title) {
            (Some(line), title) => {
                if let Some(state) = state {
                    line.state = state;
                }
                if let Some(title) = title {
                    line.title = title;
                }
            }
            (None, title) => {
                let Some(title) = title.or_else(|| update.tool_name.map(str::to_owned)) else {
                    return;
                };
                self.lines.push_back(Line {
                    key: id.map(str::to_owned),
                    state: state.unwrap_or(LineState::Running),
                    title,
                });
                self.tool_calls += 1;
                while self.lines.len() > MAX_LINES {
                    self.lines.pop_front();
                }
                self.text.clear();
                self.head.clear();
            }
        }
        self.dirty = true;
    }

    pub(super) fn apply_child<'a>(&mut self, update: impl Into<UpdateView<'a>>) {
        let update = update.into();
        match update.session_update {
            Some("tool_call") => self.apply_tool(update, true),
            Some("tool_call_update") => self.apply_tool(update, false),
            Some("agent_message_chunk") => {
                if let Some(text) = update.chunk_text() {
                    self.text.push_str(text);
                    let room = MAX_RESULT_BUFFER.saturating_sub(self.head.chars().count());
                    self.head.extend(text.chars().take(room));
                    let excess = self.text.chars().count().saturating_sub(MAX_TEXT_BUFFER);
                    if excess > 0 {
                        self.text = self.text.chars().skip(excess).collect();
                    }
                    self.dirty = true;
                    if !text.is_empty() {
                        self.last_kind = Kind::Text;
                    }
                }
            }
            Some("agent_thought_chunk") => self.last_kind = Kind::Thought,
            _ => {}
        }
    }

    /// Builds the session notice about the end; shown above the composer, not in the
    /// thread.
    pub(super) fn finish_notice(&self, failed: bool, now: Instant) -> Value {
        let name = self.name();
        json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": self.session,
                "update": {
                    "sessionUpdate": "notice",
                    "severity": if failed { "warning" } else { "info" },
                    "title": if failed { format!("{name} stopped") } else { format!("{name} finished") },
                    "description": format!("{}, {}", self.calls(), self.elapsed(now)),
                },
            },
        })
    }

    /// Builds the update that puts the sent entry back to running: the subagent was not done.
    pub(super) fn resume_entry(&self) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": self.session,
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": self.entry_id(),
                    "title": format!("{} resumed", self.name()),
                    "status": "in_progress",
                    "content": [{"type": "content", "content": {"type": "text", "text": "resumed"}}],
                },
            },
        })
    }

    /// Builds the "finished" entry: a new tool call at the end of the thread the
    /// first time, an update of that same tool call after it was sent.
    pub(super) fn finish_entry(&mut self, failed: bool) -> EntryOut {
        self.finishes += 1;
        let name = self.name();
        let title = if failed {
            format!("{name} stopped")
        } else {
            format!("{name} finished")
        };
        let mut text = self.calls();
        let last = self.result();
        if !last.is_empty() {
            text.push('\n');
            text.push_str(&last);
        }
        let status = if failed { "failed" } else { "completed" };
        let mut update = json!({
            "sessionUpdate": if self.entry_sent { "tool_call_update" } else { "tool_call" },
            "toolCallId": self.entry_id(),
            "title": title,
            "status": status,
            "content": [{"type": "content", "content": {"type": "text", "text": text}}],
        });
        let msg = |update: Value| {
            json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {"sessionId": self.session, "update": update},
            })
        };
        if self.entry_sent {
            EntryOut::Update(msg(update))
        } else {
            update["kind"] = json!("other");
            EntryOut::Append(msg(update))
        }
    }

    /// Returns the time since the subagent last did anything pons could see.
    pub(super) fn silence(&self, now: Instant) -> Duration {
        self.last_activity()
            .map_or(Duration::ZERO, |t| now.saturating_duration_since(t))
    }

    /// Returns the time since launch, frozen at the end; whole seconds, and for a running
    /// card rounded down to [`ELAPSED_STEP`].
    pub(super) fn elapsed(&self, now: Instant) -> String {
        let Some(start) = self.launched else {
            return fmt_elapsed(0);
        };
        let end = self.finished_at.unwrap_or(now);
        let secs = end.saturating_duration_since(start).as_secs();
        fmt_elapsed(if self.done.is_some() {
            secs
        } else {
            secs - secs % ELAPSED_STEP
        })
    }

    /// Returns what the subagent is called: the card's description, else `Agent`.
    pub(super) fn name(&self) -> &str {
        self.title
            .as_deref()
            .filter(|t| !matches!(*t, "Task" | "Agent"))
            .unwrap_or("Agent")
    }

    /// Returns the roster name: the input label once the input is known, else the card name.
    pub(super) fn roster_name(&self) -> String {
        match &self.input {
            Some(input) => subagent_label(input, self.name()),
            None => self.name().to_owned(),
        }
    }

    pub(super) fn note_input<'a>(&mut self, update: impl Into<UpdateView<'a>>) {
        if let Some(input) = update.into().input_object()
            && self.input.as_ref() != Some(input)
        {
            self.input = Some(input.clone());
        }
    }

    pub(super) fn note_title<'a>(&mut self, update: impl Into<UpdateView<'a>>) {
        if let Some(title) = update.into().title.map(one_line).filter(|t| !t.is_empty()) {
            self.title = Some(clip_head(&title, MAX_TITLE_CHARS));
        }
    }

    pub(super) fn new(session: &str, parent: &str) -> Self {
        Self {
            session: session.to_owned(),
            parent: parent.to_owned(),
            lines: VecDeque::new(),
            text: String::new(),
            head: String::new(),
            base: Vec::new(),
            dirty: false,
            title_dirty: false,
            last_emit: None,
            background: false,
            launched: None,
            last_child: None,
            done: None,
            finished_at: None,
            tool_calls: 0,
            footer: None,
            title: None,
            last_title: None,
            finishes: 0,
            open_tools: HashSet::new(),
            last_kind: Kind::Launched,
            launched_by: None,
            entry_sent: false,
            provisional: false,
            hidden: false,
            input: None,
            monitors: HashMap::new(),
        }
    }
}
