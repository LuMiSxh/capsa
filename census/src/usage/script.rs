// SPDX-License-Identifier: MPL-2.0
//! Reads acus's `script.jsonl`, the runs of saved scripts and of inline programs. Saved runs are
//! counted per name and scope; inline runs have no name and are counted apart.

use super::guard::{Stamp, log_lines, project_of};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One line of acus's `script.jsonl`. The envelope's `level` and `event` are ignored, and a line
/// that does not parse is skipped.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ScriptEntry {
    pub ts: u64,
    pub name: String,
    pub version: u32,
    pub scope: String,
    pub cwd: Option<String>,
    pub ms: u64,
    pub ok: bool,
    /// Set on an inline run, which has no saved name.
    pub inline: bool,
    /// Host calls an inline run made; a saved run logs none.
    pub calls: u64,
}

/// The runs and timing of one script in one scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScriptStat {
    pub name: String,
    pub scope: String,
    pub runs: u64,
    pub failures: u64,
    /// The middle run in milliseconds, the later of the two middle runs when there is an even count.
    pub median_ms: u64,
    /// Unix seconds of the latest run.
    pub last_ts: u64,
    /// The last part of the most common working directory, as `census usage` names projects.
    pub repo: String,
}

/// The script log acus writes in its state directory, beside `guard.jsonl`.
pub fn log_path() -> Option<PathBuf> {
    capsa_core::paths::log_file("acus", "script.jsonl")
}

/// Reads the entries of the script log at or after `since` (unix seconds), the rotated file first.
/// With `project`, only the entries whose working directory contains that text.
pub fn read_log(since: u64, project: Option<&str>) -> Vec<ScriptEntry> {
    let Some(path) = log_path() else {
        return Vec::new();
    };
    // Lines are streamed, and each is parsed in full only when its time is in range.
    [path.with_extension("jsonl.old"), path]
        .iter()
        .filter_map(|p| std::fs::File::open(p).ok())
        .flat_map(log_lines)
        .filter(|l| serde_json::from_str::<Stamp>(l).is_ok_and(|s| s.ts >= since))
        .filter_map(|l| serde_json::from_str::<ScriptEntry>(&l).ok())
        .filter(|e| project.is_none_or(|p| e.cwd.as_deref().unwrap_or_default().contains(p)))
        .collect()
}

/// The runs of each script and scope, the most runs first.
pub fn by_script(entries: &[ScriptEntry]) -> Vec<ScriptStat> {
    let mut groups: BTreeMap<(&str, &str), Vec<&ScriptEntry>> = BTreeMap::new();
    for e in entries.iter().filter(|e| !e.inline) {
        groups
            .entry((e.name.as_str(), e.scope.as_str()))
            .or_default()
            .push(e);
    }
    let mut stats: Vec<ScriptStat> = groups
        .into_iter()
        .map(|((name, scope), runs)| {
            let mut times: Vec<u64> = runs.iter().map(|e| e.ms).collect();
            times.sort_unstable();
            let (cwd, _) =
                cwds(&runs).into_iter().fold(
                    ("", 0),
                    |best, (cwd, n)| if n > best.1 { (cwd, n) } else { best },
                );
            ScriptStat {
                name: name.to_owned(),
                scope: scope.to_owned(),
                runs: runs.len() as u64,
                failures: runs.iter().filter(|e| !e.ok).count() as u64,
                median_ms: times.get(times.len() / 2).copied().unwrap_or_default(),
                last_ts: runs.iter().map(|e| e.ts).max().unwrap_or_default(),
                repo: project_of(Some(cwd)).to_owned(),
            }
        })
        .collect();
    stats.sort_by(|a, b| {
        b.runs
            .cmp(&a.runs)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.scope.cmp(&b.scope))
    });
    stats
}

/// The inline runs: how many, how many failed, the middle duration and the host calls they made.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct InlineStat {
    pub runs: u64,
    pub failures: u64,
    /// The middle run in milliseconds, the later one when there is an even count.
    pub median_ms: u64,
    /// Host calls made by all inline runs together.
    pub calls: u64,
}

/// Counts the inline runs in `entries`.
pub fn inline_stat(entries: &[ScriptEntry]) -> InlineStat {
    let inline: Vec<&ScriptEntry> = entries.iter().filter(|e| e.inline).collect();
    let mut times: Vec<u64> = inline.iter().map(|e| e.ms).collect();
    times.sort_unstable();
    InlineStat {
        runs: inline.len() as u64,
        failures: inline.iter().filter(|e| !e.ok).count() as u64,
        median_ms: times.get(times.len() / 2).copied().unwrap_or_default(),
        calls: inline.iter().map(|e| e.calls).sum(),
    }
}

/// The runs of each working directory, with an empty one counted as `""`.
fn cwds<'a>(runs: &[&'a ScriptEntry]) -> BTreeMap<&'a str, u64> {
    let mut cwds: BTreeMap<&str, u64> = BTreeMap::new();
    for e in runs {
        *cwds
            .entry(e.cwd.as_deref().unwrap_or_default())
            .or_default() += 1;
    }
    cwds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(line: &str) -> ScriptEntry {
        serde_json::from_str(line).unwrap()
    }

    #[test]
    fn inline_runs_are_counted_apart_from_saved_scripts() {
        let entries = [
            entry(r#"{"event":"run","name":"greet","version":1,"scope":"repo","ms":10,"ok":true}"#),
            entry(r#"{"event":"run","inline":true,"ms":30,"ok":false,"calls":4}"#),
            entry(r#"{"event":"run","inline":true,"ms":20,"ok":true,"calls":2}"#),
        ];
        let stats = by_script(&entries);
        assert_eq!(stats.len(), 1);
        assert_eq!((stats[0].name.as_str(), stats[0].runs), ("greet", 1));
        assert_eq!(
            inline_stat(&entries),
            InlineStat {
                runs: 2,
                failures: 1,
                median_ms: 30,
                calls: 6,
            }
        );
    }
}
