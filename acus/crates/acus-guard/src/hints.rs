// SPDX-License-Identifier: MPL-2.0
//! Owns the throttling of the pure hints (`Verdict::Hint`), which would repeat on every call of a
//! rule. The per-session counts live in the state files of `guard-hints`, next to guard.jsonl.

use crate::GuardConfig;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Sets how long a state file may go unmodified before it is deleted.
const KEEP: Duration = Duration::from_secs(2 * 86_400);
const DAY: Duration = Duration::from_secs(86_400);
/// Names the file whose modification time records the last prune. Session ids cannot hold a dot,
/// so no session uses this name.
const MARKER: &str = ".pruned";
/// Sets the longest `show` waits for the lock of a session.
const WAIT: Duration = Duration::from_millis(50);
/// Sets the age (`STALE`) from which a lock file belongs to a crashed process; a holder keeps it
/// for milliseconds.
const STALE: Duration = Duration::from_secs(2);

// Std only: the MSRV predates `File::lock`.
/// Holds an exclusive lock on a session. The file holds its owner's nonce and is removed on drop
/// only while it still does, so a lock that was broken as stale never deletes its successor's.
struct Lock {
    path: PathBuf,
    nonce: String,
}

/// Returns a value no other process or acquisition uses: pid and nanoseconds.
fn nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

/// Removes the lock at `path` if it is stale. The file is first renamed to a name of its own, so
/// only one breaker gets it and none removes a lock created after the age check. A renamed file
/// that turns out fresh was re-created by someone else in between: it is linked back (failing
/// harmlessly if yet another lock exists) and the lock stays.
fn break_stale(path: &Path) {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}.brk", nonce()));
    let tmp = PathBuf::from(name);
    if std::fs::rename(path, &tmp).is_err() {
        return;
    }
    let stale = std::fs::metadata(&tmp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > STALE);
    if !stale {
        let _ = std::fs::hard_link(&tmp, path);
    }
    let _ = std::fs::remove_file(&tmp);
}

impl Lock {
    /// Returns `None` if another process holds it for all of `WAIT`, or it cannot be created.
    fn acquire(state: &Path) -> Option<Self> {
        let path = state.with_extension("lock");
        std::fs::create_dir_all(path.parent()?).ok()?;
        let nonce = nonce();
        let start = Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    let _ = f.write_all(nonce.as_bytes());
                    return Some(Self { path, nonce });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > STALE);
                    if stale {
                        break_stale(&path);
                        continue;
                    }
                }
                Err(_) => return None,
            }
            if start.elapsed() >= WAIT {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|s| s == self.nonce) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
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
    prune_if_due(&cfg.state_dir);
    let Some(_lock) = Lock::acquire(&path) else {
        return true;
    };
    let Ok(mut counts) = read(&path) else {
        return true;
    };
    let n = counts.entry(rule.to_owned()).or_insert(0);
    *n += 1;
    let n = *n;
    if write(&path, &counts).is_err() {
        return true;
    }
    n % every as u64 == 1
}

/// Forgets the counts of `session`, so its hints show again.
pub fn reset(cfg: &GuardConfig, session: &str) {
    if let Some(path) = state_file(cfg, session) {
        // Removed even when the lock times out: a reset must not leave old counts.
        let _lock = Lock::acquire(&path);
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
/// when the last pass ran, so a hook that is not due pays for one stat call.
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

/// Deletes the state files not modified for two days.
fn prune(dir: &Path) {
    let (Ok(entries), Some(cutoff)) = (std::fs::read_dir(dir), SystemTime::now().checked_sub(KEEP))
    else {
        return;
    };
    for e in entries.flatten() {
        if e.metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t < cutoff)
        {
            let _ = std::fs::remove_file(e.path());
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
    fn stale_lock_is_broken_and_fresh_one_survives() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("s");
        let lock = dir.path().join("s.lock");
        std::fs::write(&lock, "dead").unwrap();
        age(&lock, STALE * 2);
        let held = Lock::acquire(&state).expect("stale lock is broken");
        // A breaker that renames a fresh lock puts it back.
        break_stale(&lock);
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), held.nonce);
        assert!(Lock::acquire(&state).is_none(), "fresh lock is respected");
        drop(held);
        assert!(!lock.exists());
    }

    #[test]
    fn drop_keeps_a_lock_it_no_longer_owns() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("s");
        let lock = dir.path().join("s.lock");
        let held = Lock::acquire(&state).unwrap();
        std::fs::write(&lock, "successor").unwrap();
        drop(held);
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), "successor");
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
