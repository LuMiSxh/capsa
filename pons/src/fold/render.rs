// SPDX-License-Identifier: MPL-2.0
//! Rendering of a folded subagent entry: the card text, its roster line and its result.

use super::*;

impl FoldEntry {
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
}
