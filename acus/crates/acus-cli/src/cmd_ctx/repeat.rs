// SPDX-License-Identifier: MPL-2.0
//! Counts identical failures of one command in one session, so `acus ctx` can tell an agent that
//! the same build keeps failing the same way.

use crate::out::clip;
use capsa_core::paths::state_dir;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A run this long after the previous run of its command starts a new count; older state is
/// ignored.
const WINDOW_SECS: u64 = 30 * 60;
/// The count from which the output leads with the repeat line.
const LEAD_FROM: u32 = 3;
/// Command records kept per session file; the oldest goes first.
const MAX_RUNS: usize = 16;
/// Session files not written for this long are removed when another file is written.
const STALE: Duration = Duration::from_secs(24 * 60 * 60);
/// Longest key line the repeat line shows.
const KEY_MAX: usize = 200;

/// Lines that name a failure: errors, failed tests, panics and the marks test runners print.
#[expect(clippy::unwrap_used, reason = "a constant pattern")]
static FAILURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)error|fail|panic|exception|assertion|[✗✘×]").unwrap());

/// The runs of one session, oldest first.
#[derive(Default, Deserialize, Serialize)]
struct Session {
    runs: Vec<Run>,
}

/// The latest run of one command in one directory.
#[derive(Deserialize, Serialize)]
struct Run {
    command: String,
    cwd: String,
    fingerprint: String,
    key: String,
    count: u32,
    at: u64,
}

/// A failure that repeated `count` times in a row, and its key line.
pub(super) struct Streak {
    pub(super) count: u32,
    pub(super) key: String,
}

impl Streak {
    /// Returns the line that leads the output: the ordinal of the failure and its key line.
    pub(super) fn lead(&self) -> String {
        format!(
            "… {} identical failure of this command in this session, same key line: {}",
            ordinal(self.count),
            self.key
        )
    }
}

/// Records how a command run ended and returns the streak once the same failure has repeated
/// `LEAD_FROM` times. A success clears the command's count. A state file that cannot be read or
/// written only loses the repeat line.
pub(super) fn note(cwd: &Path, command: &str, code: i32, lines: &[String]) -> Option<Streak> {
    note_at(&session_file(), cwd, command, code, lines, unix_now())
}

/// Records a run at `now` in `path`; `note` supplies the session file and the clock.
fn note_at(
    path: &Path,
    cwd: &Path,
    command: &str,
    code: i32,
    lines: &[String],
    now: u64,
) -> Option<Streak> {
    let cwd = cwd.to_string_lossy().into_owned();
    let mut session = read(path);
    session
        .runs
        .retain(|r| now.saturating_sub(r.at) <= WINDOW_SECS);
    let prev = session
        .runs
        .iter()
        .position(|r| r.command == command && r.cwd == cwd);
    if code == 0 {
        if let Some(i) = prev {
            session.runs.remove(i);
            write(path, &session);
        }
        return None;
    }
    let (fingerprint, key) = failure(code, lines);
    let count = match prev.map(|i| session.runs.remove(i)) {
        Some(p) if p.fingerprint == fingerprint => p.count + 1,
        _ => 1,
    };
    session.runs.push(Run {
        command: command.to_owned(),
        cwd,
        fingerprint,
        key: key.clone(),
        count,
        at: now,
    });
    if session.runs.len() > MAX_RUNS {
        session.runs.remove(0);
    }
    write(path, &session);
    (count >= LEAD_FROM).then_some(Streak { count, key })
}

/// Returns the fingerprint of a failed run and its key line. The fingerprint hashes the exit code
/// and the failure lines, so timings and progress lines do not change it. Without any failure
/// line, the last output line stands in for them.
fn failure(code: i32, lines: &[String]) -> (String, String) {
    let mut marked: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|l| FAILURE.is_match(l))
        .collect();
    if marked.is_empty() {
        marked.extend(
            lines
                .iter()
                .rev()
                .map(String::as_str)
                .find(|l| !l.trim().is_empty()),
        );
    }
    let key = marked
        .first()
        .map_or_else(|| "(no output)".to_owned(), |l| clip(l.trim(), 0, KEY_MAX));
    let fingerprint = format!("{:016x}", fnv1a(&format!("{code}\n{}", marked.join("\n"))));
    (fingerprint, key)
}

/// FNV-1a, which keeps its values across Rust releases; `DefaultHasher` does not promise that, and
/// a stored fingerprint must still match after an upgrade.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Returns the state file of the Claude Code session. Claude Code sets `CLAUDE_CODE_SESSION_ID`
/// for the commands it runs. Without it every run shares one file, and records still match only
/// the same command in the same directory.
fn session_file() -> PathBuf {
    let id = std::env::var("CLAUDE_CODE_SESSION_ID")
        .ok()
        .filter(|id| valid_id(id))
        .unwrap_or_else(|| "shared".to_owned());
    state_dir("acus").join("ctx").join(format!("{id}.json"))
}

/// A session id is a UUID; anything else is not used in a file name.
fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Reads a session file; a missing or unreadable one has no runs.
fn read(path: &Path) -> Session {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Writes the session through a temp file, so a concurrent reader never sees half of it.
fn write(path: &Path, session: &Session) {
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_string(session) else {
        return;
    };
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    if std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .is_err()
    {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    remove_stale(dir, path);
}

/// Removes the session files that were not written for `STALE`, so the directory stays small.
fn remove_stale(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        if path.as_path() != keep && age.is_some_and(|a| a > STALE) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Returns the number with its English ordinal suffix, as in `3rd` or `11th`.
fn ordinal(n: u32) -> String {
    let suffix = match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::{WINDOW_SECS, failure, note_at, ordinal};
    use std::path::Path;

    fn run_at(path: &Path, now: u64) -> Option<super::Streak> {
        let lines = ["error: mismatched".to_owned()];
        note_at(path, Path::new("/proj"), "cargo build", 1, &lines, now)
    }

    #[test]
    fn a_record_older_than_the_window_restarts_the_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        assert!(run_at(&path, 1_000).is_none());
        assert!(run_at(&path, 1_000 + WINDOW_SECS).is_none());
        assert!(
            run_at(&path, 1_000 + 2 * WINDOW_SECS + 1).is_none(),
            "a gap longer than the window starts a new count"
        );
        assert!(run_at(&path, 1_000 + 2 * WINDOW_SECS + 2).is_none());
        let streak = run_at(&path, 1_000 + 2 * WINDOW_SECS + 3).expect("the third count leads");
        assert_eq!(streak.count, 3);
    }

    #[test]
    fn ordinals_follow_english_suffixes() {
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(4), "4th");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(21), "21st");
        assert_eq!(ordinal(102), "102nd");
    }

    #[test]
    fn fingerprint_ignores_timings_but_not_the_error() {
        let run =
            |secs: &str, err: &str| failure(1, &[format!("Finished in {secs}s"), err.to_owned()]);
        let mismatch = "error[E0308]: mismatched types";
        assert_eq!(run("1.2", mismatch), run("9.8", mismatch));
        assert_ne!(
            run("1.2", mismatch).0,
            run("1.2", "error[E0432]: unresolved").0
        );
    }

    #[test]
    fn key_line_is_the_first_failure_line_or_the_last_output_line() {
        let lines = [
            "build ok".to_owned(),
            "error: a".to_owned(),
            "error: b".to_owned(),
        ];
        assert_eq!(failure(1, &lines).1, "error: a");
        assert_eq!(failure(2, &["done".to_owned()]).1, "done");
        assert_eq!(failure(2, &[]).1, "(no output)");
    }

    #[test]
    fn a_session_id_that_is_not_plain_falls_back_to_shared() {
        assert!(!super::valid_id("../x"));
        assert!(!super::valid_id("a/b"));
        assert!(!super::valid_id(""));
        assert!(super::valid_id("repeat-session"));
    }
}
