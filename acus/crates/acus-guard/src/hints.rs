// SPDX-License-Identifier: MPL-2.0
//! Owns the throttling of the pure hints (`Verdict::Hint`), which would repeat on every call of a
//! rule. The per-session counts live in the state files of `guard-hints`, in `GuardConfig`'s
//! `state_dir`.

use crate::GuardConfig;
use capsa_core::lock::{self, Lock};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Sets how long a state file may go unmodified before it is deleted.
const KEEP: Duration = Duration::from_secs(2 * 86_400);
const DAY: Duration = Duration::from_secs(86_400);
/// Names the file whose modification time records the last prune. Session ids cannot hold a dot,
/// so no session uses this name.
const MARKER: &str = ".pruned";
/// Sets the longest `show` waits for the lock of a session.
const WAIT: Duration = Duration::from_millis(50);

/// Takes the lock of a session, or `None` when another process holds it for all of `WAIT` or the
/// lock file cannot be opened.
fn acquire(state: &Path) -> Option<Lock> {
    let path = state.with_extension("lock");
    std::fs::create_dir_all(path.parent()?).ok()?;
    lock::exclusive(&path, WAIT).ok().flatten()
}
// Hooks run in parallel, so the read-modify-write holds the session lock. A state file that cannot
// be read or written shows the hint, so the throttle never hides a hint it cannot count.
/// Reports whether this occurrence of the pure hint of `rule` shows: occurrence 1, N+1, 2N+1… for
/// `[guard] hint_every` = N. The occurrence is counted either way.
pub fn show(cfg: &GuardConfig, session: &str, rule: &str) -> bool {
    let every = cfg.hint_every.max(1);
    if every == 1 {
        return true;
    }
    let Some(path) = state_file(cfg, session) else {
        return true;
    };
    let Some(_lock) = acquire(&path) else {
        return true;
    };
    let Ok(mut counts) = read(&path) else {
        return true;
    };
    let n = counts.entry(rule.to_owned()).or_insert(0);
    *n += 1;
    let n = *n;
    let show = n % every as u64 == 1;
    if show {
        prune_if_due(&cfg.state_dir);
    }
    if write(&path, &counts).is_err() {
        return true;
    }
    show
}

/// Forgets the counts of `session`, so its hints show again.
pub fn reset(cfg: &GuardConfig, session: &str) {
    if let Some(path) = state_file(cfg, session) {
        // Removed even when the lock times out: a reset must not leave old counts.
        let _lock = acquire(&path);
        let _ = std::fs::remove_file(path);
    }
}

/// Returns the file of a session. Only letters, digits, `-` and `_` name one, so an id cannot
/// leave the directory.
fn state_file(cfg: &GuardConfig, session: &str) -> Option<PathBuf> {
    let plain = !session.is_empty()
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then(|| cfg.state_dir.join(session))
}

fn read(path: &Path) -> std::io::Result<BTreeMap<String, u64>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text
            .lines()
            .filter_map(|l| {
                let (rule, n) = l.split_once(' ')?;
                Some((rule.to_owned(), n.trim().parse().ok()?))
            })
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e),
    }
}

/// Writes a temporary file and renames it over `path`, so a process crash leaves the old or the
/// new state. There is no fsync: a power loss can lose the count, which only shows a hint again.
fn write(path: &Path, counts: &BTreeMap<String, u64>) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = std::fs::File::create(&tmp)?;
    for (rule, n) in counts {
        writeln!(f, "{rule} {n}")?;
    }
    std::fs::rename(&tmp, path)
}

/// Prunes the state files of `dir` at most once a day. The modification time of the marker says
/// when the last pass ran, so only a hint that shows pays for the stat call.
fn prune_if_due(dir: &Path) {
    let last = std::fs::metadata(dir.join(MARKER))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok());
    if last.is_some_and(|age| age < DAY) {
        return;
    }
    if std::fs::write(dir.join(MARKER), b"").is_ok() {
        prune(dir);
    }
}

/// Deletes the state files not modified for two days, and the lock files of sessions whose state
/// file is gone or just as old. A lock file keeps its age while the session runs, so its state
/// file decides.
fn prune(dir: &Path) {
    let (Ok(entries), Some(cutoff)) = (std::fs::read_dir(dir), SystemTime::now().checked_sub(KEEP))
    else {
        return;
    };
    let expired = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t < cutoff)
    };
    for e in entries.flatten() {
        let path = e.path();
        let idle = if path.extension().is_some_and(|x| x == "lock") {
            let state = path.with_extension("");
            expired(&path) && (!state.exists() || expired(&state))
        } else {
            expired(&path)
        };
        if idle {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age(path: &Path, by: Duration) {
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - by).unwrap();
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("s");
        let held = acquire(&state).expect("free lock");
        assert!(acquire(&state).is_none(), "a held lock is respected");
        drop(held);
        assert!(acquire(&state).is_some());
    }

    #[test]
    fn lock_file_from_an_older_version_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s.lock"), "123-456").unwrap();
        assert!(acquire(&dir.path().join("s")).is_some());
    }

    #[test]
    fn prune_removes_idle_locks_and_leftovers_but_keeps_active_ones() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, old: bool| {
            let p = dir.path().join(name);
            std::fs::write(&p, "a 1\n").unwrap();
            if old {
                age(&p, KEEP * 2);
            }
            p
        };
        let idle = file("idle.lock", true);
        file("idle", true);
        let orphan = file("orphan.lock", true);
        let active = file("active.lock", true);
        file("active", false);
        let brk = file("idle.lock.1-2.brk", true);
        prune(dir.path());
        assert!(!idle.exists() && !orphan.exists() && !brk.exists());
        assert!(active.exists(), "a lock of an active session stays");
    }

    #[test]
    fn prune_waits_for_a_hint_to_show() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::GuardConfig {
            hint_every: 2,
            state_dir: dir.path().to_path_buf(),
            log_path: dir.path().join("guard.jsonl"),
        };
        let marker = dir.path().join(MARKER);
        assert!(show(&cfg, "s", "grep"), "the first occurrence shows");
        age(&marker, DAY * 2);
        assert!(!show(&cfg, "s", "grep"), "the second occurrence is quiet");
        let quiet = marker.metadata().unwrap().modified().unwrap().elapsed();
        assert!(quiet.unwrap() > DAY, "a quiet hint does not prune");
        assert!(show(&cfg, "s", "grep"), "the third occurrence shows");
        let shown = marker.metadata().unwrap().modified().unwrap().elapsed();
        assert!(shown.unwrap() < DAY, "a shown hint prunes");
    }

    #[test]
    fn prune_runs_at_most_once_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let marker = dir.path().join(MARKER);
        std::fs::write(&old, "a 1\n").unwrap();
        age(&old, KEEP * 2);
        prune_if_due(dir.path());
        assert!(!old.exists(), "without a marker the first pass is due");
        assert!(marker.exists());
        std::fs::write(&old, "a 1\n").unwrap();
        age(&old, KEEP * 2);
        prune_if_due(dir.path());
        assert!(old.exists(), "a marker from today skips the pass");
        age(&marker, DAY * 2);
        prune_if_due(dir.path());
        assert!(
            !old.exists(),
            "a marker older than a day makes the pass due"
        );
    }
}
