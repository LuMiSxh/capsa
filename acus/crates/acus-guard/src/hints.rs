// SPDX-License-Identifier: MPL-2.0
//! Owns the throttling of the pure hints (`Verdict::Hint`), which would repeat on every call of a
//! rule. The per-session counts live in the state files of `guard-hints`, in `GuardConfig`'s
//! `state_dir`.

use crate::{GuardConfig, follow};
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
/// Names the line of a session's state file that holds its block count and the unix time of its
/// last block or start. No rule is named `blocks`, so the line is never taken for a rule's counts.
pub(crate) const BLOCKS: &str = "blocks";

/// Takes the lock of a session, or `None` when another process holds it for all of `WAIT` or the
/// lock file cannot be opened.
pub(crate) fn acquire(state: &Path) -> Option<Lock> {
    let path = state.with_extension("lock");
    std::fs::create_dir_all(path.parent()?).ok()?;
    lock::exclusive(&path, WAIT).ok().flatten()
}
// Hooks run in parallel, so the read-modify-write holds the session lock. A state file that cannot
// be read or written shows the hint, so the throttle never hides a hint it cannot count.
/// Reports whether this occurrence of the pure hint of `rule` shows: occurrence 1, then once the
/// calls since the last shown one reach the interval of the rule. The interval is `[guard]
/// hint_every` = N, unless census publishes a current follow rate for the rule, which moves it
/// between `hint_every_min` and `hint_every_max`. The occurrence is counted either way.
///
/// The bulletin is read only when the calls since the last shown hint have reached the minimum and
/// not yet the maximum, so most calls never touch it.
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
    let Ok(mut counts) = read(&path, every as u64) else {
        return true;
    };
    let count = counts.entry(rule.to_owned()).or_default();
    count.n += 1;
    let gap = count.n.saturating_sub(count.last);
    let lo = cfg.hint_min.clamp(1, every) as u64;
    let hi = cfg.hint_max.max(every) as u64;
    let show = count.n == 1
        || gap >= hi
        || (gap >= lo
            && gap
                >= follow::interval(every, cfg.hint_min, cfg.hint_max, follow::rate(rule)) as u64);
    if show {
        count.last = count.n;
        prune_if_due(&cfg.state_dir);
    }
    if write(&path, &counts).is_err() {
        return true;
    }
    show
}

/// Forgets the hint counts of `session`, so its hints show again. The block count stays, since a
/// compaction keeps the session.
pub fn reset(cfg: &GuardConfig, session: &str) {
    edit_session(cfg, session, |counts| {
        counts.retain(|rule, _| rule == BLOCKS);
    });
}

/// Runs `edit` on the counts of `session` under its lock, then writes them back. A lock that times
/// out does not stop the write, so no count is skipped. Returns `None` when the session has no
/// state file or the file cannot be read; an unreadable file is left as it is.
pub(crate) fn edit_session<T>(
    cfg: &GuardConfig,
    session: &str,
    edit: impl FnOnce(&mut BTreeMap<String, Count>) -> T,
) -> Option<T> {
    let path = state_file(cfg, session)?;
    let _lock = acquire(&path);
    let mut counts = read(&path, 1).ok()?;
    let out = edit(&mut counts);
    let _ = write(&path, &counts);
    Some(out)
}

/// Returns the file of a session. Only letters, digits, `-` and `_` name one, so an id cannot
/// leave the directory.
pub(crate) fn state_file(cfg: &GuardConfig, session: &str) -> Option<PathBuf> {
    let plain = !session.is_empty()
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then(|| cfg.state_dir.join(session))
}

/// The calls of one rule in a session, and the number of the last one that showed. For `BLOCKS`,
/// the blocks and the time of the last one.
#[derive(Default)]
pub(crate) struct Count {
    pub(crate) n: u64,
    pub(crate) last: u64,
}

/// Reads the counts of a session. A line from an older version has no `last`; it gets the call
/// that showed under the old `n % every == 1` rule, so the next hint falls where that rule put it.
pub(crate) fn read(path: &Path, every: u64) -> std::io::Result<BTreeMap<String, Count>> {
    let every = every.max(1);
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text
            .lines()
            .filter_map(|l| {
                let mut f = l.split_whitespace();
                let rule = f.next()?.to_owned();
                let n: u64 = f.next()?.parse().ok()?;
                let last: u64 = match f.next() {
                    Some(s) => s.parse().ok()?,
                    None => n.saturating_sub(n.saturating_sub(1) % every),
                };
                Some((rule, Count { n, last }))
            })
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e),
    }
}

/// Writes a temporary file and renames it over `path`, so a process crash leaves the old or the
/// new state. There is no fsync: a power loss can lose the count, which only shows a hint again.
pub(crate) fn write(path: &Path, counts: &BTreeMap<String, Count>) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = std::fs::File::create(&tmp)?;
    for (rule, c) in counts {
        writeln!(f, "{rule} {} {}", c.n, c.last)?;
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
            hint_min: 1,
            hint_max: 8,
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

    #[test]
    fn reset_keeps_the_block_count_and_drops_the_hint_counts() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = GuardConfig {
            hint_every: 10,
            hint_min: 1,
            hint_max: 40,
            state_dir: dir.path().to_owned(),
            log_path: dir.path().join("guard.jsonl"),
        };
        edit_session(&cfg, "s", |counts| {
            counts.entry(BLOCKS.to_owned()).or_default().n = 3;
            counts.entry("read-cat".to_owned()).or_default().n = 4;
        })
        .unwrap();
        reset(&cfg, "s");
        let counts = read(&dir.path().join("s"), 1).unwrap();
        assert_eq!(counts.get(BLOCKS).map(|c| c.n), Some(3));
        assert!(!counts.contains_key("read-cat"));
    }

    #[test]
    fn an_old_line_puts_the_next_hint_where_the_old_rule_did() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s");
        let last = |n: u64, every: u64| {
            std::fs::write(&path, format!("grep {n}\n")).unwrap();
            read(&path, every).unwrap()["grep"].last
        };
        assert_eq!(last(1, 3), 1);
        assert_eq!(last(4, 3), 4);
        assert_eq!(last(5, 3), 4, "the old rule showed at the fourth call");
        assert_eq!(last(7, 3), 7);
    }

    #[test]
    fn a_last_ahead_of_the_count_does_not_underflow() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::GuardConfig {
            hint_every: 2,
            hint_min: 1,
            hint_max: 8,
            state_dir: dir.path().to_path_buf(),
            log_path: dir.path().join("guard.jsonl"),
        };
        std::fs::write(dir.path().join("s"), "grep 2 9\n").unwrap();
        assert!(
            !show(&cfg, "s", "grep"),
            "a last ahead of the count stays quiet"
        );
    }
}
