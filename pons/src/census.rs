// SPDX-License-Identifier: MPL-2.0
//! Reads the subscription limits in the adapter's rate-limit events, for the census file.

use std::time::{SystemTime, UNIX_EPOCH};

use capsa_core::census_snapshot::{Snapshot, Source, Window};
use serde_json::Value;

/// The `_meta` key under which claude-agent-acp forwards the SDK's rate limit info.
const RATE_LIMIT_KEY: &str = "_claude/rateLimit";

/// Returns the limits that one agent line carries, stamped with `t` (Unix seconds).
///
/// Only a `usage_update` with a rate-limit event has them. `unifiedWindows` gives the
/// five-hour and seven-day windows; without it, the top-level `rateLimitType` window is read.
/// Other window types and malformed fields are skipped.
pub fn extract(msg: &Value, t: u64) -> Vec<Snapshot> {
    let Some(update) = msg.pointer("/params/update") else {
        return Vec::new();
    };
    if update.get("sessionUpdate").and_then(Value::as_str) != Some("usage_update") {
        return Vec::new();
    }
    let Some(info) = update
        .get("_meta")
        .and_then(|meta| meta.get(RATE_LIMIT_KEY))
    else {
        return Vec::new();
    };
    match info.get("unifiedWindows") {
        Some(windows) => ["five_hour", "seven_day"]
            .into_iter()
            .filter_map(|name| snapshot(window_named(name)?, windows.get(name)?, t))
            .collect(),
        None => info
            .get("rateLimitType")
            .and_then(Value::as_str)
            .and_then(window_named)
            .and_then(|window| snapshot(window, info, t))
            .into_iter()
            .collect(),
    }
}

fn window_named(name: &str) -> Option<Window> {
    match name {
        "five_hour" => Some(Window::FiveHour),
        "seven_day" => Some(Window::SevenDay),
        _ => None,
    }
}

/// Reads `utilization`, a fraction, and `resetsAt` from one window object.
fn snapshot(window: Window, obj: &Value, t: u64) -> Option<Snapshot> {
    let utilization = obj.get("utilization")?.as_f64()?;
    if !utilization.is_finite() || utilization < 0.0 {
        return None;
    }
    Some(Snapshot {
        t,
        source: Source::Pons,
        window,
        used_percentage: utilization * 100.0,
        resets_at: obj.get("resetsAt")?.as_u64()?,
    })
}

/// Keeps the last value of each window, so a value that repeats is not recorded again.
#[derive(Debug, Default)]
pub struct Limits {
    last: Vec<Snapshot>,
}

impl Limits {
    /// Returns the limits of `msg` whose value differs from the last one seen for their window.
    pub fn observe(&mut self, msg: &Value, t: u64) -> Vec<Snapshot> {
        extract(msg, t)
            .into_iter()
            .filter(|snapshot| self.changed(snapshot))
            .collect()
    }

    /// Stores `snapshot` as its window's last value and tells whether the value changed.
    fn changed(&mut self, snapshot: &Snapshot) -> bool {
        match self
            .last
            .iter()
            .position(|last| last.window == snapshot.window)
        {
            Some(index) => {
                let last = &mut self.last[index];
                let changed = last.used_percentage != snapshot.used_percentage
                    || last.resets_at != snapshot.resets_at;
                *last = *snapshot;
                changed
            }
            None => {
                self.last.push(*snapshot);
                true
            }
        }
    }
}

/// Returns the current Unix time in seconds, or zero when the clock is set before 1970.
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const SAMPLE: &str = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"usage_update","used":1,"size":2,"_meta":{"_claude/rateLimit":{"status":"allowed","resetsAt":1793530800,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.45,"resetsAt":1793530800},"seven_day":{"utilization":0.05,"resetsAt":1794106800}}}}}}}"#;

    fn rate_limit(info: Value) -> Value {
        let mut msg = json!({"params": {"update": {"sessionUpdate": "usage_update",
            "_meta": {}}}});
        msg["params"]["update"]["_meta"][RATE_LIMIT_KEY] = info;
        msg
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn reads_both_unified_windows_as_percentages() {
        let msg: Value = serde_json::from_str(SAMPLE).unwrap();
        let snapshots = extract(&msg, 100);
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].window, Window::FiveHour);
        assert!(near(snapshots[0].used_percentage, 45.0));
        assert_eq!(snapshots[0].resets_at, 1_793_530_800);
        assert_eq!(snapshots[1].window, Window::SevenDay);
        assert!(near(snapshots[1].used_percentage, 5.0));
        assert_eq!(snapshots[1].resets_at, 1_794_106_800);
        assert!(
            snapshots
                .iter()
                .all(|s| s.t == 100 && s.source == Source::Pons)
        );
    }

    #[test]
    fn falls_back_to_the_top_level_window_without_unified_windows() {
        let msg = rate_limit(json!({"rateLimitType": "seven_day", "utilization": 0.3,
            "resetsAt": 1794106800}));
        let snapshots = extract(&msg, 7);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].window, Window::SevenDay);
        assert!(near(snapshots[0].used_percentage, 30.0));
        assert_eq!(snapshots[0].resets_at, 1_794_106_800);
    }

    #[test]
    fn other_window_types_are_ignored() {
        let opus = rate_limit(json!({"unifiedWindows": {"seven_day_opus":
            {"utilization": 0.9, "resetsAt": 1}}}));
        assert!(extract(&opus, 0).is_empty());
        let overage = rate_limit(json!({"rateLimitType": "overage", "utilization": 0.9,
            "resetsAt": 1}));
        assert!(extract(&overage, 0).is_empty());
    }

    #[test]
    fn malformed_rate_limit_meta_is_ignored() {
        let bad = [
            json!("five_hour"),
            json!({"unifiedWindows": "soon"}),
            json!({"unifiedWindows": {"five_hour": {"utilization": "45%", "resetsAt": 1}}}),
            json!({"unifiedWindows": {"five_hour": {"utilization": -0.1, "resetsAt": 1}}}),
            json!({"unifiedWindows": {"five_hour": {"utilization": 0.4}}}),
            json!({"rateLimitType": "five_hour", "resetsAt": 1}),
        ];
        for info in bad {
            let msg = rate_limit(info);
            assert!(extract(&msg, 0).is_empty(), "{msg}");
        }
    }

    #[test]
    fn a_malformed_window_does_not_hide_the_other_one() {
        let msg = rate_limit(json!({"unifiedWindows": {
            "five_hour": {"utilization": "x"},
            "seven_day": {"utilization": 0.05, "resetsAt": 1794106800}}}));
        let snapshots = extract(&msg, 0);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].window, Window::SevenDay);
    }

    #[test]
    fn other_updates_and_lines_without_limits_are_ignored() {
        let chunk = json!({"params": {"update": {"sessionUpdate": "agent_message_chunk",
            "_meta": {"_claude/rateLimit": {"rateLimitType": "five_hour",
                "utilization": 0.4, "resetsAt": 1}}}}});
        assert!(extract(&chunk, 0).is_empty());
        let bare = json!({"params": {"update": {"sessionUpdate": "usage_update", "used": 1}}});
        assert!(extract(&bare, 0).is_empty());
        assert!(extract(&json!({"id": 1, "result": {}}), 0).is_empty());
    }

    #[test]
    fn a_repeated_value_is_observed_once_per_window() {
        let msg: Value = serde_json::from_str(SAMPLE).unwrap();
        let mut limits = Limits::default();
        assert_eq!(limits.observe(&msg, 1).len(), 2);
        assert!(limits.observe(&msg, 2).is_empty());

        let mut moved = msg.clone();
        moved["params"]["update"]["_meta"]["_claude/rateLimit"]["unifiedWindows"]["five_hour"]["utilization"] =
            json!(0.46);
        let changed = limits.observe(&moved, 3);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].window, Window::FiveHour);
        assert!(near(changed[0].used_percentage, 46.0));
        assert!(limits.observe(&moved, 4).is_empty());
    }
}
