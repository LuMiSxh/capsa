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
    pub(super) fn silent_finish(
        &self,
        now: Instant,
        idle: Duration,
        cap: Duration,
        text_visible: bool,
    ) -> Option<&'static str> {
        if !self.is_running_background() || !self.open_tools.is_empty() {
            return None;
        }
        let silent = self.silence(now);
        if text_visible {
            let needed = match self.last_kind {
                Kind::Text => idle,
                Kind::ToolResult => idle.max(TOOL_RESULT_IDLE),
                _ => return None,
            };
            (!idle.is_zero() && silent >= needed).then_some("idle")
        } else {
            (!cap.is_zero() && silent >= cap).then_some("quiet-cap")
        }
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

    /// Returns valid ACP tool call content: the parent's own content, then the log.
    pub(super) fn content(&self) -> Vec<Value> {
        let mut content = self.base.clone();
        content.push(json!({
            "type": "content",
            "content": { "type": "text", "text": self.render() },
        }));
        content
    }

    /// Renders a compact text log: the last tool lines plus the tail of the agent text.
    pub fn render(&self) -> String {
        // A finished card shows the final message; a running one its tail.
        let finished = self.done.is_some().then(|| self.result());
        let text = match finished.filter(|r| !r.is_empty()) {
            Some(result) => result,
            None => clip_tail(one_line(&self.text).trim(), MAX_TEXT_CHARS),
        };
        let reserve = usize::from(!text.is_empty()) + usize::from(self.footer.is_some());
        let keep = MAX_LINES - reserve;
        let skip = self.lines.len().saturating_sub(keep);
        let mut out: Vec<String> = self
            .lines
            .iter()
            .skip(skip)
            .map(|l| format!("{}: {}", l.state.word(), l.title))
            .collect();
        if !text.is_empty() {
            out.push(format!("agent: {text}"));
        }
        if let Some(footer) = &self.footer {
            out.push(footer.clone());
        } else if out.is_empty() && self.background {
            out.push("running in the background".to_owned());
        }
        out.join("\n")
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

    pub(super) fn apply_tool(&mut self, update: &Value, is_call: bool) {
        let id = str_field(update, "toolCallId");
        let status = str_field(update, "status");
        let title = str_field(update, "title")
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
                let Some(title) = title.or_else(|| tool_name(update).map(str::to_owned)) else {
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

    pub(super) fn apply_child(&mut self, update: &Value) {
        match str_field(update, "sessionUpdate") {
            Some("tool_call") => self.apply_tool(update, true),
            Some("tool_call_update") => self.apply_tool(update, false),
            Some("agent_message_chunk") => {
                if let Some(text) = update.pointer("/content/text").and_then(Value::as_str) {
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

    pub(super) fn entry_id(&self) -> String {
        format!("{ENTRY_PREFIX}{}", self.parent)
    }

    /// Returns the subagent's final agent message: the last contiguous agent text,
    /// blank runs squeezed, at most [`MAX_RESULT_CHARS`] characters of its
    /// head. Empty when the subagent said nothing since its last tool call.
    pub(super) fn result(&self) -> String {
        let mut out: Vec<&str> = Vec::new();
        for line in self.head.lines().map(str::trim_end) {
            if line.is_empty() && out.last().is_none_or(|l| l.is_empty()) {
                continue;
            }
            out.push(line);
        }
        clip_head(out.join("\n").trim(), MAX_RESULT_CHARS)
    }

    pub(super) fn roster_item(&self, now: Instant) -> RosterItem {
        let name = self.roster_name();
        match self.done {
            None => {
                let action = self.last_action();
                let mut stats = format!("{}, {}", self.calls(), self.elapsed(now));
                if let Some(quiet) = self.quiet_label(now) {
                    stats.push_str(&format!(", {quiet}"));
                }
                RosterItem {
                    content: format!(
                        "{name}: {} ({stats})",
                        action.as_deref().unwrap_or("starting")
                    ),
                    status: if action.is_some() {
                        "in_progress"
                    } else {
                        "pending"
                    },
                }
            }
            Some(done) => RosterItem {
                content: format!(
                    "{name}: {} ({}, {})",
                    if done.failed { "stopped" } else { "done" },
                    self.calls(),
                    self.elapsed(now)
                ),
                status: "completed",
            },
        }
    }

    /// Returns the live title of a background card; `None` for any other card, whose
    /// lifecycle belongs to the adapter.
    pub(super) fn card_title(&self, now: Instant) -> Option<String> {
        if !self.background {
            return None;
        }
        let name = self.name();
        Some(match self.done {
            None => {
                let mut title = format!("{name}: running {}", self.elapsed(now));
                if self.tool_calls > 0 {
                    title.push_str(&format!(", {}", self.calls()));
                    if let Some(action) = self.last_action() {
                        title.push_str(&format!(", last: {action}"));
                    }
                }
                if let Some(quiet) = self.quiet_label(now) {
                    title.push_str(&format!(", {quiet}"));
                }
                title
            }
            Some(done) => format!(
                "{name}: {}, {}, {}",
                if done.failed { "stopped" } else { "done" },
                self.calls(),
                self.elapsed(now)
            ),
        })
    }

    pub(super) fn last_action(&self) -> Option<String> {
        self.lines
            .back()
            .map(|l| clip_head(&l.title, MAX_ACTION_CHARS))
    }

    /// Returns `quiet 35s` once the running subagent has been silent a while, in
    /// steps of [`ELAPSED_STEP`].
    pub(super) fn quiet_label(&self, now: Instant) -> Option<String> {
        let secs = self.silence(now).as_secs();
        (secs >= QUIET_SHOWN).then(|| format!("quiet {}", fmt_elapsed(secs - secs % ELAPSED_STEP)))
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

    pub(super) fn note_input(&mut self, update: &Value) {
        if let Some(input) = update
            .get("rawInput")
            .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
        {
            self.input = Some(input.clone());
        }
    }

    pub(super) fn note_title(&mut self, update: &Value) {
        if let Some(title) = str_field(update, "title")
            .map(one_line)
            .filter(|t| !t.is_empty())
        {
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
        }
    }
}
