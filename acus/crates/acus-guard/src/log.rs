// SPDX-License-Identifier: MPL-2.0
//! Owns the decision log that `acus usage --guard` reads: one JSON line per decision.

use crate::{GuardConfig, LOG_CALL_MAX, Verdict};
use serde_json::Value;
use std::io::Write;

/// Appends one decision as a JSON line; a log that cannot be written is skipped.
pub fn append(cfg: &GuardConfig, hook: &Value, v: &Verdict) {
    let path = &cfg.log_path;
    // Start over past 4 MB, keeping one old generation.
    if std::fs::metadata(path).is_ok_and(|m| m.len() > 4 * 1024 * 1024) {
        let _ = std::fs::rename(path, path.with_extension("jsonl.old"));
    }
    let input = &hook["tool_input"];
    let call: String = ["command", "file_path", "pattern"]
        .iter()
        .find_map(|k| input[k].as_str())
        .unwrap_or_default()
        .chars()
        .take(LOG_CALL_MAX)
        .collect();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let entry = LogEntry {
        ts,
        session: hook["session_id"].as_str().map(str::to_owned),
        cwd: hook["cwd"].as_str().map(str::to_owned),
        tool: hook["tool_name"].as_str().map(str::to_owned),
        rule: v.rule().as_str().to_owned(),
        kind: v.kind().to_owned(),
        call,
        cmd: None,
    };
    let Ok(line) = serde_json::to_string(&entry) else {
        return;
    };
    let _ = path.parent().map(std::fs::create_dir_all);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        // One write call per line: parallel hooks append to the same file, and a newline written
        // apart from its line lets another hook's line land between them.
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

/// Holds one line of guard.jsonl. The fields keep the order they are written in, so a line read
/// here and written back is the same line. `cmd` is only in replay files (`--replay`).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LogEntry {
    pub ts: u64,
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
    fn log_entry_round_trips_and_reads_old_lines() {
        let old = r#"{"ts":1,"session":"s","cwd":"/p","tool":"Bash","rule":"grep","kind":"deny","call":"grep x"}"#;
        let e: LogEntry = serde_json::from_str(old).unwrap();
        assert_eq!(serde_json::to_string(&e).unwrap(), old);
        let replay: LogEntry = serde_json::from_str(r#"{"cmd":"cat a.rs"}"#).unwrap();
        assert_eq!(replay.cmd.as_deref(), Some("cat a.rs"));
    }

    #[test]
    fn concurrent_logged_lines_are_each_one_json_object() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = GuardConfig {
            hint_every: 10,
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
                        append(&cfg, &hook, &verdict);
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
}
