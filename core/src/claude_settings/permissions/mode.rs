// SPDX-License-Identifier: MPL-2.0
//! Claude Code's permission mode, and the per-session file a hook leaves it in. Claude Code
//! exposes the mode to hooks only, never to the commands it runs, so a tool that wants to follow it
//! records it from a hook and reads it back by the session id.

use crate::fs::write_cache;
use serde_json::json;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a session file lives after its last write.
const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

/// The permission mode Claude Code reports as `permission_mode`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Default,
    Plan,
    AcceptEdits,
    Auto,
    DontAsk,
    BypassPermissions,
}

impl Mode {
    /// Returns the mode named `name` as Claude Code writes it, or `None` for an unknown name.
    pub fn parse(name: &str) -> Option<Mode> {
        Some(match name {
            "default" => Mode::Default,
            "plan" => Mode::Plan,
            "acceptEdits" => Mode::AcceptEdits,
            "auto" => Mode::Auto,
            "dontAsk" => Mode::DontAsk,
            "bypassPermissions" => Mode::BypassPermissions,
            _ => return None,
        })
    }

    /// Returns the name Claude Code uses for the mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Default => "default",
            Mode::Plan => "plan",
            Mode::AcceptEdits => "acceptEdits",
            Mode::Auto => "auto",
            Mode::DontAsk => "dontAsk",
            Mode::BypassPermissions => "bypassPermissions",
        }
    }
}

/// The directory that holds one small file per Claude Code session, `<session>.json` with
/// `{"mode": "...", "at": <unix seconds>}`.
pub struct ModeStore {
    dir: PathBuf,
}

impl ModeStore {
    /// Returns the store in `dir`; nothing is created until a mode is recorded.
    pub fn new(dir: PathBuf) -> Self {
        ModeStore { dir }
    }

    /// Returns the store in a tool's state directory.
    pub fn in_state_dir(tool: &str) -> Self {
        Self::new(crate::paths::state_dir(tool).join("permission-mode"))
    }

    /// Returns the mode last recorded for `session`; a missing or unreadable file gives `None`.
    pub fn read(&self, session: &str) -> Option<Mode> {
        let text = std::fs::read_to_string(self.file(session)?).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        Mode::parse(value["mode"].as_str()?)
    }

    /// Records `mode` for `session` and returns whether a file was written: nothing is written
    /// when the recorded mode is already `mode`. Files older than seven days are removed when a
    /// file is written.
    ///
    /// # Errors
    /// Fails when the directory or the file cannot be written.
    pub fn record(&self, session: &str, mode: Mode) -> io::Result<bool> {
        let Some(file) = self.file(session) else {
            return Ok(false);
        };
        if self.read(session) == Some(mode) {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.dir)?;
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let text = json!({"mode": mode.as_str(), "at": at}).to_string();
        write_cache(&file, text.as_bytes())?;
        self.prune();
        Ok(true)
    }

    /// Returns the file of `session`, or `None` for an empty id. Characters outside letters,
    /// digits, `-` and `_` become `_`, so an id can never name another directory.
    fn file(&self, session: &str) -> Option<PathBuf> {
        if session.is_empty() {
            return None;
        }
        let name: String = session
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Some(self.dir.join(format!("{name}.json")))
    }

    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > KEEP);
            if old {
                // A file that another session removed first is already gone.
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_reads_and_skips_an_unchanged_mode() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModeStore::new(dir.path().join("modes"));
        assert_eq!(store.read("s1"), None);
        assert!(store.record("s1", Mode::Plan).unwrap());
        assert_eq!(store.read("s1"), Some(Mode::Plan));
        assert!(!store.record("s1", Mode::Plan).unwrap());
        assert!(store.record("s1", Mode::DontAsk).unwrap());
        assert_eq!(store.read("s1"), Some(Mode::DontAsk));
        assert_eq!(store.read("s2"), None);
    }

    #[test]
    fn a_session_id_cannot_leave_the_directory_and_an_empty_one_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModeStore::new(dir.path().join("modes"));
        assert!(store.record("../evil", Mode::Auto).unwrap());
        assert!(dir.path().join("modes/___evil.json").exists());
        assert!(!store.record("", Mode::Auto).unwrap());
    }

    #[test]
    fn old_files_are_pruned_when_a_file_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModeStore::new(dir.path().join("modes"));
        store.record("old", Mode::Plan).unwrap();
        let old = dir.path().join("modes/old.json");
        let week = SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(week)
            .unwrap();
        store.record("new", Mode::Auto).unwrap();
        assert!(!old.exists());
        assert_eq!(store.read("new"), Some(Mode::Auto));
    }

    #[test]
    fn a_garbled_file_reads_as_no_mode() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModeStore::new(dir.path().to_path_buf());
        std::fs::write(dir.path().join("s.json"), "{nope").unwrap();
        assert_eq!(store.read("s"), None);
    }
}
