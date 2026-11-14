// SPDX-License-Identifier: MPL-2.0
//! Owns the follow rate of each guard rule: how often the agent stopped repeating a call that the
//! guard refused, hinted or rewrote. It works from the log alone, which keeps no hint text.

use super::guard::GuardEntry;
use std::collections::{BTreeMap, HashMap};

/// A decision counts as not followed when its rule fires again in the same session within this many
/// seconds. The log keeps no hint text, so this stands in for "ran the suggested acus command".
pub const WINDOW_SECS: u64 = 10 * 60;

/// A rule shows its follow rate only after this many observed decisions.
pub const MIN_SAMPLE: u64 = 5;

/// The observed decisions of one rule, and those that were not repeated in their window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Follow {
    pub observed: u64,
    pub followed: u64,
}

impl Follow {
    /// Returns the share of observed decisions that were followed, or `None` below `MIN_SAMPLE`.
    pub fn rate(self) -> Option<f32> {
        (self.observed >= MIN_SAMPLE).then(|| self.followed as f32 / self.observed as f32)
    }
}

/// Returns the follow counts of each rule, for the decisions that refuse, hint or rewrite a call.
///
/// A decision is followed when no decision of its rule comes in the same session within
/// `WINDOW_SECS` after it. It is observed once the log holds a line at least `WINDOW_SECS` after it,
/// so a repeat that is still to come is never counted as followed. Escapes are not suggestions, and
/// a line without a session cannot be joined to a later one; both are left out.
pub fn follow_rates(entries: &[GuardEntry]) -> BTreeMap<&str, Follow> {
    let mut out: BTreeMap<&str, Follow> = BTreeMap::new();
    let Some(horizon) = entries.iter().map(|e| e.ts).max() else {
        return out;
    };
    let mut fires: HashMap<(&str, &str), Vec<u64>> = HashMap::new();
    for e in entries {
        if let Some(session) = e.session.as_deref() {
            fires
                .entry((session, e.rule.as_str()))
                .or_default()
                .push(e.ts);
        }
    }
    for times in fires.values_mut() {
        times.sort_unstable();
    }
    for e in entries {
        let suggests = matches!(e.kind.as_str(), "deny" | "hint" | "rewrite");
        let Some(session) = e.session.as_deref() else {
            continue;
        };
        if !suggests || e.ts + WINDOW_SECS > horizon {
            continue;
        }
        let Some(times) = fires.get(&(session, e.rule.as_str())) else {
            continue;
        };
        // Same-second lines are not repeats: the search starts after every line at `e.ts`.
        let next = times.partition_point(|&t| t <= e.ts);
        let repeated = times.get(next).is_some_and(|&t| t <= e.ts + WINDOW_SECS);
        let slot = out.entry(e.rule.as_str()).or_default();
        slot.observed += 1;
        slot.followed += u64::from(!repeated);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ts: u64, session: Option<&str>, rule: &str, kind: &str) -> GuardEntry {
        GuardEntry {
            ts,
            session: session.map(str::to_owned),
            rule: rule.to_owned(),
            kind: kind.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn a_repeat_of_the_rule_in_the_same_session_is_not_followed() {
        let log = [
            line(100, Some("s1"), "grep", "hint"),
            line(200, Some("s1"), "grep", "deny"),
            line(1000, Some("s2"), "sed", "escape"),
        ];
        let rates = follow_rates(&log);
        assert_eq!(
            rates["grep"],
            Follow {
                observed: 2,
                followed: 1
            }
        );
        assert!(!rates.contains_key("sed"));
    }

    #[test]
    fn a_repeat_in_another_session_or_after_the_window_is_not_a_repeat() {
        let log = [
            line(100, Some("s1"), "grep", "hint"),
            line(150, Some("s2"), "grep", "hint"),
            line(1000, Some("s1"), "grep", "hint"),
            line(1000, Some("s3"), "sed", "escape"),
        ];
        // The last hint is 900 s after the first one, and its own window runs past the horizon.
        assert_eq!(
            follow_rates(&log)["grep"],
            Follow {
                observed: 2,
                followed: 2
            }
        );
    }

    #[test]
    fn a_decision_without_a_line_past_its_window_is_not_observed() {
        let log = [
            line(100, Some("s1"), "grep", "hint"),
            line(200, Some("s1"), "grep", "deny"),
        ];
        assert!(follow_rates(&log).is_empty());
    }

    #[test]
    fn escapes_and_lines_without_a_session_are_left_out() {
        let log = [
            line(100, Some("s1"), "grep", "escape"),
            line(100, None, "grep", "deny"),
            line(1000, Some("s1"), "sed", "escape"),
        ];
        assert!(follow_rates(&log).is_empty());
    }

    #[test]
    fn a_rate_waits_for_the_minimum_sample() {
        assert_eq!(
            Follow {
                observed: 4,
                followed: 4
            }
            .rate(),
            None
        );
        assert_eq!(
            Follow {
                observed: 5,
                followed: 4
            }
            .rate(),
            Some(0.8)
        );
    }
}
