// SPDX-License-Identifier: MPL-2.0
//! Background subagent cards and the detection of their finish.

use super::*;

impl Folds {
    /// Returns the sessions that have background subagents, running or finished.
    pub fn background_sessions(&self) -> Vec<String> {
        let mut ids: Vec<&str> = self
            .entries
            .values()
            .filter(|e| e.background && !e.hidden)
            .map(|e| e.session.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().map(str::to_owned).collect()
    }

    /// Finishes one background card. When the entry is gone (forgotten or
    /// retired since the caller looked), it notes that and returns `None`.
    pub(super) fn finish_background(
        &mut self,
        parent: &str,
        failed: bool,
        by: Option<Instant>,
        reason: &'static str,
        confirmed: bool,
        now: Instant,
    ) -> Option<Finished> {
        let (idle, cap) = (self.idle, self.max_quiet);
        let finish_entry = self.finish_entry;
        let Some(entry) = self.entries.get_mut(parent) else {
            self.notes.push(format!(
                "{reason}: background entry {parent} is gone; nothing finished"
            ));
            return None;
        };
        // A finished card's monitors end with it: later activity must not hold it open.
        entry.monitors.clear();
        if entry.done.is_some() {
            // Already shown as done by silence: only confirm it.
            entry.provisional = !confirmed;
            return Some(Finished {
                session: entry.session.clone(),
                parent: parent.to_owned(),
                reason,
                confirmed,
                update: None,
                entry: None,
                notice: entry.finish_notice(failed, now),
            });
        }
        let calls = entry.calls();
        let footer = match reason {
            "stop" => format!("stopped - {calls}"),
            "idle" => format!("done (no activity for {}s) - {calls}", idle.as_secs_f32()),
            "quiet-cap" => format!("done (no activity for {}s) - {calls}", cap.as_secs_f32()),
            _ => format!("done - {calls}"),
        };
        let update = entry.finish(failed, by, footer, now);
        entry.provisional = !confirmed;
        let announce = finish_entry.then(|| entry.finish_entry(failed));
        Some(Finished {
            session: entry.session.clone(),
            parent: parent.to_owned(),
            reason,
            confirmed,
            update: Some(update),
            entry: announce,
            notice: entry.finish_notice(failed, now),
        })
    }

    /// Moves the cards launched by prompt `from` to prompt `to`: the adapter
    /// handed the turn over to a newer prompt instead of ending it.
    pub fn reassign_launched_by(&mut self, session: &str, from: &str, to: &str) {
        for e in self.entries.values_mut() {
            if e.session == session && e.launched_by.as_deref() == Some(from) {
                e.launched_by = Some(to.to_owned());
            }
        }
    }

    /// Finishes the cards prompt `prompt` launched: the adapter keeps a prompt open until
    /// every subagent launched in its turn is gone, so each background card it
    /// launched is done (`failed` when the turn was cancelled).
    pub fn finish_launched_by(
        &mut self,
        session: &str,
        prompt: &str,
        failed: bool,
        now: Instant,
    ) -> Vec<Finished> {
        let mut ids: Vec<String> = self
            .entries
            .values()
            .filter(|e| {
                e.session == session
                    && e.launched_by.as_deref() == Some(prompt)
                    && (e.is_running_background() || e.is_provisional())
            })
            .map(|e| e.parent.clone())
            .collect();
        ids.sort();
        ids.iter()
            .filter_map(|id| self.finish_background(id, failed, None, "prompt response", true, now))
            .collect()
    }

    /// Finishes the background cards of `session` after a real Stop: running ones fail.
    /// Cards that were only provisionally finished stay as they are, but are
    /// confirmed (nothing runs any more).
    pub fn stop_session(&mut self, session: &str, now: Instant) -> Vec<Finished> {
        for e in self.entries.values_mut() {
            if e.session == session && e.is_provisional() {
                e.provisional = false;
            }
        }
        let mut ids = self.running_background(session);
        ids.sort();
        ids.iter()
            .filter_map(|id| self.finish_background(id, true, None, "stop", true, now))
            .collect()
    }

    /// Finish background cards by their signals. A matured `task-notification`
    /// ends the best candidate (agent text last, no tool open, quiet longest;
    /// a subagent with a tool open is never picked, one whose `Monitor` runs is
    /// picked only when no other qualifies, and none qualifying ends
    /// nothing). Silence ends a card only by the rule in the module docs.
    pub fn poll(&mut self, now: Instant) -> Vec<Finished> {
        let mut out = Vec::new();
        for (session, at) in self.take_ripe_notifications(now) {
            match self.notification_candidate(&session, at, now) {
                Some(parent) => out.extend(self.finish_background(
                    &parent,
                    false,
                    Some(at),
                    "notification",
                    true,
                    now,
                )),
                None => self.notes.push(format!(
                    "task-notification in {session}: no background subagent qualifies (tools open, active since, or none running); finished nothing"
                )),
            }
        }
        let (idle, cap, visible) = (self.idle, self.max_quiet, self.text_visible);
        let mut silent: Vec<(String, &'static str)> = self
            .entries
            .values()
            .filter_map(|e| {
                e.silent_finish(now, idle, cap, visible)
                    .map(|reason| (e.parent.clone(), reason))
            })
            .collect();
        silent.sort_unstable();
        for (parent, reason) in silent {
            out.extend(self.finish_background(&parent, false, None, reason, false, now));
        }
        out
    }

    /// Moves the markers that waited `settle` out of the queue and returns them.
    fn take_ripe_notifications(&mut self, now: Instant) -> Vec<(String, Instant)> {
        let settle = self.settle;
        let (ripe, waiting) = std::mem::take(&mut self.notifications)
            .into_iter()
            .partition(|(_, t)| now.saturating_duration_since(*t) >= settle);
        self.notifications = waiting;
        ripe
    }

    /// Picks the card a marker at `at` most likely ended. A card whose `Monitor` runs is
    /// picked only when no other card qualifies.
    fn notification_candidate(&self, session: &str, at: Instant, now: Instant) -> Option<String> {
        [false, true].into_iter().find_map(|monitored| {
            self.entries
                .values()
                .filter(|e| {
                    e.session == session && e.marker_candidate(at) && e.monitoring(now) == monitored
                })
                .min_by_key(|e| {
                    (
                        u8::from(e.last_kind != Kind::Text),
                        e.last_activity(),
                        e.parent.as_str(),
                    )
                })
                .map(|e| e.parent.clone())
        })
    }

    /// Lists the background subagents of `session` that may still be working: running
    /// ones and those only provisionally finished (not yet hidden).
    pub fn live_background(&self, session: &str) -> Vec<String> {
        self.entries
            .values()
            .filter(|e| {
                e.session == session
                    && (e.is_running_background() || (e.is_provisional() && !e.hidden))
            })
            .map(|e| e.parent.clone())
            .collect()
    }

    /// Lists the background subagents of `session` that have not been finished.
    pub fn running_background(&self, session: &str) -> Vec<String> {
        self.entries
            .values()
            .filter(|e| e.session == session && e.is_running_background())
            .map(|e| e.parent.clone())
            .collect()
    }

    /// Returns the full update of every running background subagent of `session`,
    /// status and title included. Used after a cancel was dropped: the client was
    /// told the turn was cancelled and has finished those cards, so they are
    /// asserted again.
    pub fn reassert_running(&mut self, session: &str, now: Instant) -> Vec<Value> {
        let mut ids = self.running_background(session);
        ids.sort();
        ids.iter()
            .filter_map(|id| {
                let e = self.entries.get_mut(id)?;
                e.dirty = true;
                e.take_emit(now)
            })
            .collect()
    }

    /// Records a `task-notification` marker: some background task ended.
    pub fn note_notification(&mut self, session: &str, now: Instant) {
        self.notifications.push((session.to_owned(), now));
    }

    /// Takes over a background subagent's launch, which the adapter reported as a
    /// completed tool call: returns that update rewritten to `in_progress`, with the
    /// launch blob replaced by the running log.
    /// `launched_by` is the prompt that is outstanding, if any.
    pub fn launch_background<'a>(
        &mut self,
        session: &str,
        parent: &str,
        update: impl Into<UpdateView<'a>>,
        launched_by: Option<&str>,
        now: Instant,
    ) -> Value {
        let entry = self
            .entries
            .entry(parent.to_owned())
            .or_insert_with(|| FoldEntry::new(session, parent));
        entry.background = true;
        entry.launched = Some(now);
        entry.launched_by = launched_by.map(str::to_owned);
        entry.last_kind = Kind::Launched;
        entry.done = None;
        entry.finished_at = None;
        entry.footer = None;
        entry.provisional = false;
        entry.hidden = false;
        entry.dirty = false;
        entry.title_dirty = false;
        entry.last_emit = Some(now);
        let title = entry.card_title(now);
        entry.last_title.clone_from(&title);
        let mut out = update.into().raw().clone();
        if let Some(obj) = out.as_object_mut() {
            obj.remove("rawOutput");
            obj.insert("status".into(), json!("in_progress"));
            obj.insert("content".into(), json!(entry.content()));
            if let Some(title) = title {
                obj.insert("title".into(), json!(title));
            }
        }
        out
    }
}
