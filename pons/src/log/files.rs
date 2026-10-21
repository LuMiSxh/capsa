// SPDX-License-Identifier: MPL-2.0

//! Finds the log directory and keeps only the newest logs.

use super::*;

/// Log files kept in the default directory, the current one included.
pub(super) const KEEP_LOGS: usize = 10;

/// Returns the default log directory for the OS `os` (`std::env::consts::OS`); `env`
/// reads environment variables.
pub fn default_dir(os: &str, env: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    let nonempty = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = || nonempty("HOME").unwrap_or_else(std::env::temp_dir);
    match os {
        "macos" => home().join("Library/Logs/pons"),
        "windows" => nonempty("LOCALAPPDATA")
            .unwrap_or_else(std::env::temp_dir)
            .join("pons")
            .join("logs"),
        // XDG requires an absolute path. Test for the leading "/" rather than
        // `is_absolute`, which is host-specific: "/state" is not absolute on
        // Windows, so the result would otherwise depend on the host.
        _ => nonempty("XDG_STATE_HOME")
            .filter(|p| p.starts_with("/"))
            .unwrap_or_else(|| home().join(".local/state"))
            .join("pons"),
    }
}

/// Returns the wire log for this start, or `None` when logging is off. `--no-log`
/// wins; an explicit `--log` path is used as given. Otherwise a new
/// `pons-<local time>-<pid>.log` is created in `dir` (default: `default_dir`)
/// and older ones beyond the newest `KEEP_LOGS` are removed.
pub fn target(explicit: Option<&Path>, no_log: bool, dir: Option<&Path>) -> Option<PathBuf> {
    if no_log {
        return None;
    }
    if let Some(path) = explicit {
        return Some(path.to_owned());
    }
    let dir = resolve_dir(dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "pons: wire log disabled, cannot create {}: {e}",
            dir.display()
        );
        return None;
    }
    let stamp = Local::now().format("%Y-%m-%dT%H-%M-%S");
    let path = dir.join(format!("pons-{stamp}-{}.log", std::process::id()));
    prune(&dir, KEEP_LOGS, &path);
    Some(path)
}

/// Splits a `pons-<time>-<pid>.log` name into its `(time, pid)` key, `None` for any other name.
pub(super) fn log_key(name: &str) -> Option<(&str, u32)> {
    let stem = name.strip_prefix("pons-")?.strip_suffix(".log")?;
    let (ts, pid) = stem.rsplit_once('-')?;
    let well_formed =
        ts.len() == "2026-10-14T22-06-01".len() && ts.starts_with(|c: char| c.is_ascii_digit());
    well_formed.then_some((ts, pid.parse().ok()?))
}

/// Returns the directory `dir`, or the default log directory for this OS when `None`.
pub fn resolve_dir(dir: Option<&Path>) -> PathBuf {
    dir.map_or_else(
        || default_dir(std::env::consts::OS, |k| std::env::var_os(k)),
        Path::to_owned,
    )
}

/// Lists the `pons-<time>-<pid>.log` files in `dir`, with their `(time, pid)` keys.
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

/// Removes the oldest `pons-<time>-<pid>.log` files in `dir` so that, with
/// `current` counted, at most `keep` remain. Other files are never touched.
pub(super) fn prune(dir: &Path, keep: usize, current: &Path) {
    let mut logs = logs_in(dir);
    logs.retain(|(_, path)| path.as_path() != current);
    logs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in logs.into_iter().skip(keep.saturating_sub(1)) {
        let _ = std::fs::remove_file(path);
    }
}
