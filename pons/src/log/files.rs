// SPDX-License-Identifier: MPL-2.0

//! Finds the log directory and keeps only the newest logs.

use super::*;

/// Log files kept in the default directory, the current one included.
pub(super) const KEEP_LOGS: usize = 10;

/// Returns the wire log for this start, or `None` when logging is off. `--no-log`
/// wins; an explicit `--log` path is used as given. Otherwise a new
/// `pons-<local time>-<pid>.jsonl` is created in `dir` (default: pons's state
/// directory) and older ones beyond the newest `KEEP_LOGS` are removed.
pub fn target(explicit: Option<&Path>, no_log: bool, dir: Option<&Path>) -> Option<PathBuf> {
    if no_log {
        return None;
    }
    if let Some(path) = explicit {
        return Some(path.to_owned());
    }
    let given_dir = dir.is_some();
    let dir = resolve_dir(dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "pons: wire log disabled, cannot create {}: {e}",
            dir.display()
        );
        return None;
    }
    let stamp = Local::now().format("%Y-%m-%dT%H-%M-%S");
    let name = format!("pons-{stamp}-{}.jsonl", std::process::id());
    let path = if given_dir {
        dir.join(name)
    } else {
        capsa_core::paths::log_file("pons", &name).unwrap_or_else(|| dir.join(name))
    };
    prune(&dir, KEEP_LOGS, &path);
    Some(path)
}

/// Splits a `pons-<time>-<pid>.jsonl` (or older `.log`) name into its `(time, pid)` key.
pub(super) fn log_key(name: &str) -> Option<(&str, u32)> {
    let stem = name.strip_prefix("pons-")?;
    let stem = stem
        .strip_suffix(".jsonl")
        .or_else(|| stem.strip_suffix(".log"))?;
    let (ts, pid) = stem.rsplit_once('-')?;
    let well_formed =
        ts.len() == "2026-10-14T22-06-01".len() && ts.starts_with(|c: char| c.is_ascii_digit());
    well_formed.then_some((ts, pid.parse().ok()?))
}

/// Returns the directory `dir`, or pons's state directory when `None`.
pub fn resolve_dir(dir: Option<&Path>) -> PathBuf {
    dir.map_or_else(|| capsa_core::paths::state_dir("pons"), Path::to_owned)
}

/// Lists the per-start logs in `dir`, with their `(time, pid)` keys.
pub(super) fn logs_in(dir: &Path) -> Vec<((String, u32), PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let (ts, pid) = log_key(&name)?;
            Some(((ts.to_owned(), pid), e.path()))
        })
        .collect()
}

/// Returns the newest per-start log in `dir` (default directory when `None`).
pub fn newest_log(dir: Option<&Path>) -> Option<PathBuf> {
    logs_in(&resolve_dir(dir))
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

/// Removes the oldest per-start logs in `dir` so that, with
/// `current` counted, at most `keep` remain. Other files are never touched.
pub(super) fn prune(dir: &Path, keep: usize, current: &Path) {
    let mut logs = logs_in(dir);
    logs.retain(|(_, path)| path.as_path() != current);
    logs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in logs.into_iter().skip(keep.saturating_sub(1)) {
        let _ = std::fs::remove_file(path);
    }
}
