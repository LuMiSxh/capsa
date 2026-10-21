// SPDX-License-Identifier: MPL-2.0
//! Retires finished background cards and drops the state of closed sessions.

use super::*;

impl Folds {
    pub(super) fn drain(&mut self, pick: impl Fn(&FoldEntry) -> bool, now: Instant) -> Vec<Value> {
        let mut ids: Vec<&String> = self
            .entries
            .iter()
            .filter(|(_, e)| pick(e))
            .map(|(id, _)| id)
            .collect();
        ids.sort();
        let ids: Vec<String> = ids.into_iter().cloned().collect();
        ids.iter()
            .filter_map(|id| self.entries.get_mut(id)?.take_emit(now))
            .collect()
    }

    /// Returns the roster of `session`: its background subagents in launch order.
    pub fn roster(&self, session: &str, now: Instant) -> Vec<RosterItem> {
        let mut entries: Vec<&FoldEntry> = self
            .entries
            .values()
            .filter(|e| e.session == session && e.background && !e.hidden)
            .collect();
        entries.sort_by(|a, b| (a.launched, &a.parent).cmp(&(b.launched, &b.parent)));
        entries.iter().map(|e| e.roster_item(now)).collect()
    }

    pub(super) fn prune_roots(&mut self) {
        let entries = &self.entries;
        self.child_root.retain(|_, root| entries.contains_key(root));
    }

    pub(super) fn bury(&mut self, e: FoldEntry) {
        if !e.background {
            return;
        }
        let key = e.parent.clone();
        let tomb = Tombstone {
            session: e.session.clone(),
            finishes: e.finishes,
            entry_sent: e.entry_sent,
            title: e.title,
        };
        if self.tombstones.insert(key.clone(), tomb).is_none() {
            self.tomb_order.push_back(key);
        }
        while self.tomb_order.len() > MAX_TOMBSTONES {
            if let Some(old) = self.tomb_order.pop_front() {
                self.tombstones.remove(&old);
            }
        }
    }

    /// Drops a confirmed card (leaving a tombstone); a provisional one is
    /// only hidden from the roster and keeps its log, so activity after a
    /// false finish resumes it intact. Hidden ones expire after
    /// [`HIDDEN_KEEP`].
    pub(super) fn retire_where(
        &mut self,
        pick: impl Fn(&FoldEntry) -> bool,
        now: Option<Instant>,
    ) -> Vec<(String, String)> {
        let mut gone = Vec::new();
        let mut drop_ids = Vec::new();
        for (id, e) in &mut self.entries {
            if e.done.is_none() {
                continue;
            }
            let expired = now.is_some_and(|now| {
                e.hidden
                    && e.finished_at
                        .is_some_and(|f| now.saturating_duration_since(f) >= HIDDEN_KEEP)
            });
            if expired {
                drop_ids.push(id.clone());
            } else if pick(e) && !e.hidden {
                gone.push((e.session.clone(), e.parent.clone()));
                if e.provisional {
                    e.hidden = true;
                } else {
                    drop_ids.push(id.clone());
                }
            }
        }
        for id in drop_ids {
            if let Some(e) = self.entries.remove(&id) {
                self.bury(e);
            }
        }
        if !gone.is_empty() {
            self.prune_roots();
        }
        gone.sort();
        gone
    }

    /// Retire finished background subagents after [`FINISHED_KEEP`].
    pub fn retire_expired(&mut self, now: Instant) -> Vec<(String, String)> {
        self.retire_where(
            |e| {
                e.finished_at
                    .is_some_and(|f| now.saturating_duration_since(f) >= FINISHED_KEEP)
            },
            Some(now),
        )
    }

    /// Retire finished background subagents of `session` that were already
    /// done when the turn that just ended began. Returns `(session, parent)`
    /// of the cards that left the roster.
    pub fn retire(&mut self, session: &str, turn_start: Option<Instant>) -> Vec<(String, String)> {
        let Some(start) = turn_start else {
            return Vec::new();
        };
        self.retire_where(
            |e| e.session == session && e.finished_at.is_some_and(|f| f < start),
            None,
        )
    }

    /// Forget everything about `session` (it was closed).
    pub fn forget_session(&mut self, session: &str) {
        self.entries.retain(|_, e| e.session != session);
        self.notifications.retain(|(s, _)| s != session);
        self.tombstones.retain(|_, t| t.session != session);
        let tombs = &self.tombstones;
        self.tomb_order.retain(|k| tombs.contains_key(k));
        self.prune_roots();
    }

    /// Forget the folds of `session` (the turn ended), except background
    /// subagents, which outlive the turn that launched them (running ones
    /// keep going, finished ones stay in the roster until retired).
    pub fn clear_session(&mut self, session: &str) {
        self.entries
            .retain(|_, e| e.session != session || e.background);
        self.prune_roots();
    }

    /// Emits every pending log of `session` regardless of the throttle.
    pub fn flush_session(&mut self, session: &str, now: Instant) -> Vec<Value> {
        self.drain(|e| e.session == session, now)
    }

    /// Emits every pending log whose throttle window has passed, and refreshes
    /// the elapsed and quiet time in the title of running cards (title only).
    pub fn tick(&mut self, now: Instant) -> Vec<Value> {
        let throttle = self.throttle;
        for e in self.entries.values_mut() {
            if e.is_running_background() && !e.dirty && e.card_title(now) != e.last_title {
                e.title_dirty = true;
            }
        }
        self.drain(|e| e.is_due(now, throttle), now)
    }
}
