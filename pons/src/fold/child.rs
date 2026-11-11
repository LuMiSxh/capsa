// SPDX-License-Identifier: MPL-2.0
//! Routes updates of a subagent into its folded entry.

use super::*;

impl Folds {
    /// Handles the parent's own update. When it is terminal, the pending log
    /// is returned and must be sent before the forwarded update.
    pub fn parent_update<'a>(
        &mut self,
        parent: &str,
        update: impl Into<UpdateView<'a>>,
        terminal: bool,
        now: Instant,
    ) -> Option<Value> {
        let update = update.into();
        let entry = self.entries.get_mut(parent)?;
        entry.note_title(update);
        let flushed = if terminal { entry.take_emit(now) } else { None };
        if terminal && entry.is_running_background() {
            entry.done = Some(Done {
                failed: update.status == Some("failed"),
                by: None,
            });
            entry.finished_at = Some(now);
        }
        if let Some(base) = update.content_items() {
            entry.base.clone_from(base);
            // The forwarded update replaces the card content; restore the log.
            entry.dirty = !entry.lines.is_empty();
        }
        if entry.background && update.has_title_key() {
            // The forwarded update replaced the card title; restore ours.
            entry.dirty = true;
        }
        flushed
    }

    /// Records that the "finished" entry of `parent` was appended to the thread.
    pub fn mark_entry_sent(&mut self, parent: &str) {
        if let Some(e) = self.entries.get_mut(parent) {
            e.entry_sent = true;
        }
    }

    /// Tells whether `parent` is a background card that pons has finished (and
    /// not reopened since).
    pub fn is_finished(&self, parent: &str) -> bool {
        self.entries
            .get(parent)
            .is_some_and(|e| e.background && e.done.is_some())
    }

    /// Folds a subagent update into `root`. Returns the updates to send now:
    /// the card's, unless the throttle holds it back until `tick`, and, when
    /// activity reopens a card whose entry was already sent, the entry's
    /// `resumed` update.
    pub fn child_update<'a>(
        &mut self,
        session: &str,
        root: &str,
        update: impl Into<UpdateView<'a>>,
        now: Instant,
    ) -> Vec<Value> {
        let update = update.into();
        if update.tool_call_id == Some(root) {
            // The adapter's own progress tick for the card, tagged with the
            // card as its parent. It is not a child: tracking it as a tool
            // call would leave the card "working" forever (nothing ever ends
            // it with a parent tag), so no notification could finish it.
            return Vec::new();
        }
        if update.session_update == Some("tool_call")
            && let Some(id) = update.tool_call_id
        {
            self.child_root.insert(id.to_owned(), root.to_owned());
        }
        let mut reopened = false;
        if !self.entries.contains_key(root)
            && let Some(tomb) = self.tombstones.remove(root)
        {
            // A retired card is active again: it was a background one.
            self.tomb_order.retain(|k| k != root);
            let mut e = FoldEntry::new(session, root);
            e.background = true;
            e.launched = Some(now);
            e.finishes = tomb.finishes;
            e.entry_sent = tomb.entry_sent;
            e.title = tomb.title;
            e.dirty = true;
            reopened = true;
            self.notes.push(format!(
                "background subagent {root} active again after it was retired"
            ));
            self.entries.insert(root.to_owned(), e);
        }
        let entry = self
            .entries
            .entry(root.to_owned())
            .or_insert_with(|| FoldEntry::new(session, root));
        entry.last_child = Some(now);
        if entry.background
            && let Some(done) = entry.done.take()
        {
            // Activity after the card was finished: it was not done after all.
            // A notification that ended the wrong card is up for matching again.
            if let Some(at) = done.by
                && now.saturating_duration_since(at) <= REMATCH_WINDOW
            {
                self.notifications.push((session.to_owned(), at));
            }
            self.notes.push(format!(
                "background subagent {root} active again after it finished"
            ));
            entry.footer = None;
            entry.finished_at = None;
            entry.provisional = false;
            entry.hidden = false;
            entry.dirty = true;
            entry.last_emit = None;
            reopened = true;
        }
        entry.note_monitor(update, now);
        entry.apply_child(update);
        let mut out = Vec::new();
        if entry.is_due(now, self.throttle)
            && let Some(card) = entry.take_emit(now)
        {
            out.push(card);
        }
        if reopened && entry.entry_sent {
            out.push(entry.resume_entry());
        }
        out
    }
}
