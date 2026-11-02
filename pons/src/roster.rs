// SPDX-License-Identifier: MPL-2.0
//! Owns the subagent roster shown in the ACP `plan` session update.
//! Zed replaces the whole plan on every update, so pons resends the adapter's entries
//! with its own appended.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::fold::{Folds, RosterItem};

#[derive(Debug, Default)]
struct PlanState {
    /// The adapter's latest plan update, for its `_meta` and other fields.
    template: Option<Value>,
    /// The adapter's latest plan entries.
    adapter: Vec<Value>,
    /// The entries Zed shows as far as pons knows; `None` until pons sent a
    /// plan of its own (Zed then shows what the adapter sent).
    shown: Option<Vec<Value>>,
    last_emit: Option<Instant>,
}

#[derive(Debug)]
pub struct Roster {
    interval: Duration,
    sessions: HashMap<String, PlanState>,
}

fn entry_json(item: &RosterItem) -> Value {
    json!({"content": item.content, "priority": "medium", "status": item.status})
}

/// The plan entries Zed should show: the adapter's, then the roster's.
fn merged(adapter: &[Value], items: &[RosterItem]) -> Vec<Value> {
    let mut want = adapter.to_vec();
    want.extend(items.iter().map(entry_json));
    want
}

fn plan_message(session: &str, template: Option<&Value>, entries: Vec<Value>) -> Value {
    let mut update = template
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    update["sessionUpdate"] = json!("plan");
    update["entries"] = Value::Array(entries);
    json!({"jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": session, "update": update}})
}

fn is_completed(entry: &Value) -> bool {
    entry.get("status").and_then(Value::as_str) == Some("completed")
}

impl Roster {
    /// Forgets the plan state of a closed session.
    pub fn forget(&mut self, session: &str) {
        self.sessions.remove(session);
    }

    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            sessions: HashMap::new(),
        }
    }

    /// Handles the start of a turn: Zed drops the completed entries of the plan, so the
    /// adapter's must not come back with the next pons plan.
    pub fn on_prompt(&mut self, session: &str) {
        if let Some(state) = self.sessions.get_mut(session) {
            state.adapter.retain(|e| !is_completed(e));
            if let Some(shown) = &mut state.shown {
                shown.retain(|e| !is_completed(e));
            }
        }
    }

    /// Handles a plan that the adapter sent. `Some` is the merged plan to send instead;
    /// `None` means forward the adapter's message unchanged (no subagents).
    pub fn on_adapter_plan(
        &mut self,
        session: &str,
        update: &Value,
        items: &[RosterItem],
    ) -> Option<Value> {
        let entries = update.get("entries")?.as_array()?.clone();
        let state = self.sessions.entry(session.to_owned()).or_default();
        state.template = Some(update.clone());
        state.adapter = entries;
        let mut merged = state.adapter.clone();
        merged.extend(items.iter().map(entry_json));
        state.shown = Some(merged.clone());
        if items.is_empty() {
            return None;
        }
        Some(plan_message(session, state.template.as_ref(), merged))
    }

    /// Returns the plans to send now: the roster changed (or emptied) and the throttle
    /// window has passed.
    pub fn poll(&mut self, folds: &Folds, now: Instant) -> Vec<Value> {
        let mut ids: BTreeSet<String> = self.sessions.keys().cloned().collect();
        ids.extend(folds.background_sessions());
        let mut out = Vec::new();
        for id in ids {
            let items = folds.roster(&id, now);
            let state = self.sessions.entry(id.clone()).or_default();
            let want = merged(&state.adapter, &items);
            let shown = state.shown.as_ref().unwrap_or(&state.adapter);
            if &want == shown {
                continue;
            }
            if state
                .last_emit
                .is_some_and(|t| now.saturating_duration_since(t) < self.interval)
            {
                continue;
            }
            state.shown = Some(want.clone());
            state.last_emit = Some(now);
            out.push(plan_message(&id, state.template.as_ref(), want));
        }
        out
    }

    /// Returns the plan of `session` again, although it is unchanged and within the
    /// throttle: Zed may have dropped the running entries with a cancelled turn.
    /// `None` when no subagent is listed.
    pub fn resend(&mut self, session: &str, folds: &Folds, now: Instant) -> Option<Value> {
        let items = folds.roster(session, now);
        if items.is_empty() {
            return None;
        }
        let state = self.sessions.entry(session.to_owned()).or_default();
        let want = merged(&state.adapter, &items);
        state.shown = Some(want.clone());
        state.last_emit = Some(now);
        Some(plan_message(session, state.template.as_ref(), want))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, status: &'static str) -> RosterItem {
        RosterItem {
            content: content.to_owned(),
            status,
        }
    }

    fn adapter_plan(entries: &Value) -> Value {
        json!({"sessionUpdate": "plan", "entries": entries})
    }

    fn todo(content: &str, status: &str) -> Value {
        json!({"content": content, "priority": "medium", "status": status})
    }

    #[test]
    fn plan_without_subagents_passes_through() {
        let mut r = Roster::new(Duration::from_secs(1));
        let plan = adapter_plan(&json!([todo("write tests", "pending")]));
        assert!(r.on_adapter_plan("s", &plan, &[]).is_none());
    }

    #[test]
    fn adapter_plan_is_merged_with_the_roster() {
        let mut r = Roster::new(Duration::from_secs(1));
        let plan = adapter_plan(&json!([todo("write tests", "pending")]));
        let merged = r
            .on_adapter_plan(
                "s",
                &plan,
                &[item("A: Read a (1 tool call, 0s)", "in_progress")],
            )
            .unwrap();
        assert_eq!(
            merged["params"]["update"],
            json!({"sessionUpdate": "plan", "entries": [
                todo("write tests", "pending"),
                {"content": "A: Read a (1 tool call, 0s)", "priority": "medium", "status": "in_progress"},
            ]})
        );
    }

    #[test]
    fn resend_repeats_the_plan_although_unchanged_and_throttled() {
        let t0 = Instant::now();
        let mut folds = Folds::new(Duration::ZERO);
        folds.launch_background("s", "P", &json!({"toolCallId": "P"}), Some("1"), t0);
        let mut r = Roster::new(Duration::from_secs(1));
        let plan = adapter_plan(&json!([todo("write tests", "pending")]));
        r.on_adapter_plan("s", &plan, &[]);
        let first = r.poll(&folds, t0);
        assert_eq!(first.len(), 1);
        assert!(r.poll(&folds, t0 + Duration::from_millis(100)).is_empty());
        let again = r
            .resend("s", &folds, t0 + Duration::from_millis(100))
            .expect("plan sent again");
        assert_eq!(again["params"]["update"], first[0]["params"]["update"]);
        let entries = again["params"]["update"]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], todo("write tests", "pending"));
        assert!(r.resend("s", &Folds::new(Duration::ZERO), t0).is_none());
    }

    #[test]
    fn completed_adapter_entries_are_dropped_at_the_next_turn() {
        let mut r = Roster::new(Duration::from_secs(1));
        let plan = adapter_plan(&json!([todo("a", "completed"), todo("b", "pending")]));
        r.on_adapter_plan("s", &plan, &[]);
        r.on_prompt("s");
        assert_eq!(r.sessions["s"].adapter, vec![todo("b", "pending")]);
        assert_eq!(r.sessions["s"].shown, Some(vec![todo("b", "pending")]));
    }
}
