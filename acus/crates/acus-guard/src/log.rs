// SPDX-License-Identifier: MPL-2.0
//! Owns the decision log that `census usage --guard` reads: one JSON line per decision.

use crate::{GuardConfig, LOG_CALL_MAX, Verdict};
use capsa_core::log::Level;
use serde_json::Value;

/// Past this size the log rotates to `guard.jsonl.old`, keeping one old generation.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Appends one decision as a JSON line; a line that cannot be written is returned as an error.
pub fn append(cfg: &GuardConfig, hook: &Value, v: &Verdict) -> std::io::Result<()> {
    let input = &hook["tool_input"];
    let call: String = ["command", "file_path", "pattern"]
        .iter()
        .find_map(|k| input[k].as_str())
        .unwrap_or_default()
        .chars()
        .take(LOG_CALL_MAX)
        .collect();
    let entry = LogEntry {
        session: hook["session_id"].as_str().map(str::to_owned),
        cwd: hook["cwd"].as_str().map(str::to_owned),
        tool: hook["tool_name"].as_str().map(str::to_owned),
        rule: v.rule().as_str().to_owned(),
        kind: v.kind().as_str().to_owned(),
        call,
        cmd: None,
    };
    // `append` writes each line in one call: parallel hooks share the file, and a newline written
    // apart from its line lets another hook's line land between them.
    capsa_core::log::append(&cfg.log_path, MAX_BYTES, Level::Info, "decision", &entry)
}

/// Holds the guard's fields of one guard.jsonl line. `capsa_core::log` writes `ts`, `level` and
/// `event` before them, and reading ignores them, so lines from before the envelope still parse.
/// The fields keep the order they are written in. `cmd` is only in replay files (`--replay`).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LogEntry {
    pub session: Option<String>,
    pub cwd: Option<String>,
    pub tool: Option<String>,
    pub rule: String,
    pub kind: String,
    pub call: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_entry_round_trips_its_fields_and_reads_old_lines() {
        let old = r#"{"ts":1,"session":"s","cwd":"/p","tool":"Bash","rule":"grep","kind":"deny","call":"grep x"}"#;
        let e: LogEntry = serde_json::from_str(old).unwrap();
        assert_eq!(e.call, "grep x");
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"session":"s","cwd":"/p","tool":"Bash","rule":"grep","kind":"deny","call":"grep x"}"#
        );
        let replay: LogEntry = serde_json::from_str(r#"{"cmd":"cat a.rs"}"#).unwrap();
        assert_eq!(replay.cmd.as_deref(), Some("cat a.rs"));
    }

    #[test]
    fn concurrent_logged_lines_are_each_one_json_object() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = GuardConfig {
            hint_every: 10,
            hint_min: 5,
            hint_max: 40,
            state_dir: dir.path().join("guard-hints"),
            log_path: dir.path().join("guard.jsonl"),
        };
        let prefix = "grep -n \"a\\b\" 'x\ty'\nna\u{ef}ve \u{4e2d} ";
        let command = format!("{prefix}{}", "\\\"".repeat(LOG_CALL_MAX));
        let hook = serde_json::json!({
            "session_id": "s",
            "tool_name": "Bash",
            "tool_input": { "command": command },
        });
        let verdict = Verdict::Deny(crate::Rule::Grep, "r".to_owned());
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..50 {
                        append(&cfg, &hook, &verdict).unwrap();
                    }
                });
            }
        });
        let text = std::fs::read_to_string(&cfg.log_path).unwrap();
        assert_eq!(text.lines().count(), 200);
        for line in text.lines() {
            let entry: LogEntry = serde_json::from_str(line).unwrap();
            assert_eq!(entry.call.chars().count(), LOG_CALL_MAX);
            assert!(entry.call.starts_with(prefix));
        }
    }

    #[test]
    fn replay_accepts_lines_with_and_without_the_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("guard.jsonl");
        std::fs::write(
            &path,
            "{\"ts\":1,\"session\":\"s\",\"cwd\":\"/p\",\"tool\":\"Bash\",\"rule\":\"grep\",\"kind\":\"deny\",\"call\":\"grep x\"}\n",
        )
        .unwrap();
        let cfg = GuardConfig {
            hint_every: 10,
            hint_min: 5,
            hint_max: 40,
            state_dir: dir.path().join("guard-hints"),
            log_path: path.clone(),
        };
        let hook = serde_json::json!({
            "session_id": "s",
            "tool_name": "Bash",
            "tool_input": { "command": "grep x" },
        });
        append(
            &cfg,
            &hook,
            &Verdict::Deny(crate::Rule::Grep, "r".to_owned()),
        )
        .unwrap();
        crate::replay_file(&path, 1).unwrap();
        let entries = capsa_core::log::read::<LogEntry>(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].level, None);
        assert_eq!(entries[0].event, None);
        assert_eq!(entries[1].level, Some(Level::Info));
        assert_eq!(entries[1].event.as_deref(), Some("decision"));
        assert_eq!(entries[1].fields.call, "grep x");
    }
}
