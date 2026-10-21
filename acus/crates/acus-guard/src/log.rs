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
        let _ = writeln!(f, "{line}");
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
}
