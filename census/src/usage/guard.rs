// SPDX-License-Identifier: MPL-2.0
//! Reads acus's `guard.jsonl`, the decisions the acus guard wrote, and counts them per rule and per
//! project.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

/// One line of acus's `guard.jsonl`. Fields keep acus's order, so `--json` prints them as acus
/// wrote them; unknown fields are ignored and old rule names still parse.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuardEntry {
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

/// The decisions of one rule, by kind. A kind that is neither a refusal, a hint nor a rewrite
/// counts as an escape, as the log has always been read.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Tally {
    pub deny: u64,
    pub hint: u64,
    pub escape: u64,
    pub rewrite: u64,
}

impl Tally {
    fn add(&mut self, kind: &str) {
        match kind {
            "deny" => self.deny += 1,
            "hint" => self.hint += 1,
            "rewrite" => self.rewrite += 1,
            _ => self.escape += 1,
        }
    }

    /// Returns the decisions of every kind.
    pub fn total(&self) -> u64 {
        self.deny + self.hint + self.escape + self.rewrite
    }
}

/// The decisions of one rule in one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRule {
    pub project: String,
    pub rule: String,
    pub tally: Tally,
}

/// The decision log acus writes in its state directory. `None` when no state directory is known.
pub fn log_path() -> Option<PathBuf> {
    capsa_core::paths::log_file("acus", "guard.jsonl")
}

/// Reads the entries of the guard log at or after `since` (unix seconds), the rotated file first.
/// With `project`, only the entries whose working directory contains that text. A line that does
/// not parse is skipped. Empty when no state directory is known.
pub fn read_log(since: u64, project: Option<&str>) -> Vec<GuardEntry> {
    let Some(path) = log_path() else {
        return Vec::new();
    };
    // Lines are streamed, and each is parsed in full only when its time is in range.
    [path.with_extension("jsonl.old"), path]
        .iter()
        .filter_map(|p| std::fs::File::open(p).ok())
        .flat_map(log_lines)
        .filter(|l| serde_json::from_str::<Stamp>(l).is_ok_and(|s| s.ts >= since))
        .filter_map(|l| serde_json::from_str::<GuardEntry>(&l).ok())
        .filter(|e| project.is_none_or(|p| e.cwd.as_deref().unwrap_or_default().contains(p)))
        .collect()
}

/// The decisions of each rule, in rule order.
pub fn by_rule(entries: &[GuardEntry]) -> BTreeMap<&str, Tally> {
    let mut rules: BTreeMap<&str, Tally> = BTreeMap::new();
    for e in entries {
        rules.entry(e.rule.as_str()).or_default().add(&e.kind);
    }
    rules
}

/// The decisions of each rule in each project, the most decisions first. A project is the last part
/// of the working directory, as `census usage` names the projects of its sessions.
pub fn by_project(entries: &[GuardEntry]) -> Vec<ProjectRule> {
    let mut rows: BTreeMap<(&str, &str), Tally> = BTreeMap::new();
    for e in entries {
        rows.entry((project_of(e.cwd.as_deref()), e.rule.as_str()))
            .or_default()
            .add(&e.kind);
    }
    let mut rows: Vec<ProjectRule> = rows
        .into_iter()
        .map(|((project, rule), tally)| ProjectRule {
            project: project.to_owned(),
            rule: rule.to_owned(),
            tally,
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.tally.total()));
    rows
}

fn project_of(cwd: Option<&str>) -> &str {
    cwd.map(Path::new)
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .unwrap_or("?")
}

/// The time of one guard log line, read without deserializing the rest of it.
#[derive(Deserialize)]
struct Stamp {
    #[serde(default)]
    ts: u64,
}

/// The lines of one log file, read on past a line that is not valid UTF-8. The file ends at its
/// last line or at an I/O error.
fn log_lines(file: std::fs::File) -> impl Iterator<Item = String> {
    let mut reader = std::io::BufReader::new(file);
    std::iter::from_fn(move || {
        loop {
            let mut buf = Vec::new();
            match BufRead::read_until(&mut reader, b'\n', &mut buf) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            if buf.last() == Some(&b'\n') {
                buf.pop();
            }
            if let Ok(line) = String::from_utf8(buf) {
                return Some(line);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: u64, cwd: &str, rule: &str, kind: &str) -> GuardEntry {
        GuardEntry {
            ts,
            cwd: Some(cwd.to_owned()),
            rule: rule.to_owned(),
            kind: kind.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn by_project_groups_rules_per_directory_name_most_decisions_first() {
        let entries = [
            entry(1, "/work/capsa", "grep", "deny"),
            entry(2, "/work/other", "sed", "hint"),
            entry(3, "/work/capsa", "grep", "hint"),
            entry(4, "/work/capsa", "sed", "escape"),
            entry(5, "", "grep", "rewrite"),
        ];
        let rows = by_project(&entries);
        let got: Vec<(&str, &str, u64)> = rows
            .iter()
            .map(|r| (r.project.as_str(), r.rule.as_str(), r.tally.total()))
            .collect();
        assert_eq!(
            got,
            [
                ("capsa", "grep", 2),
                ("?", "grep", 1),
                ("capsa", "sed", 1),
                ("other", "sed", 1),
            ]
        );
        assert_eq!(
            rows[0].tally,
            Tally {
                deny: 1,
                hint: 1,
                escape: 0,
                rewrite: 0
            }
        );
    }

    #[test]
    fn by_rule_counts_unknown_kinds_as_escapes() {
        let entries = [
            entry(1, "/p", "grep", "pass"),
            entry(2, "/p", "grep", "rewrite"),
        ];
        let rules = by_rule(&entries);
        assert_eq!(
            rules["grep"],
            Tally {
                deny: 0,
                hint: 0,
                escape: 1,
                rewrite: 1
            }
        );
    }
}
