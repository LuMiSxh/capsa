// SPDX-License-Identifier: MPL-2.0
//! Keeps one small typed status file per tool, so that every tool can read the others' status.

use crate::fs::write_atomic;
use crate::paths::state_dir;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Holds a value with the unix time it was recorded and how long it stays current.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamped<T> {
    /// Unix seconds at which the value was recorded.
    pub at: u64,
    /// Seconds after `at` during which the value is current.
    pub ttl: u64,
    pub value: T,
}

impl<T> Stamped<T> {
    /// Returns the value while `now` is before `at + ttl`, and `None` once it is stale.
    pub fn current(&self, now: u64) -> Option<&T> {
        (now < self.at.saturating_add(self.ttl)).then_some(&self.value)
    }
}

/// Lists what can go wrong when writing a bulletin.
#[derive(Debug, thiserror::Error)]
pub enum BulletinError {
    #[error("`{0}` is not a bulletin tool name")]
    InvalidTool(String),
    #[error("cannot encode the bulletin")]
    Encode(#[from] serde_json::Error),
    #[error("cannot write {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Replaces the bulletin of `tool` with `value`, atomically, in the capsa state directory.
///
/// Each call writes a whole file, so keep it off hot paths.
///
/// # Errors
/// Fails when `tool` is not a valid name, or when the value cannot be encoded or written.
pub fn write<T: Serialize>(tool: &str, value: &T) -> Result<(), BulletinError> {
    write_in(&dir(), tool, value)
}

/// Reads the bulletin of `tool`, or `None` when it is missing, unreadable or not a `T`.
pub fn read<T: DeserializeOwned>(tool: &str) -> Option<T> {
    read_in(&dir(), tool)
}

/// Returns the current unix time in seconds, or 0 before the epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn dir() -> PathBuf {
    state_dir("capsa").join("bulletin")
}

fn write_in<T: Serialize>(base: &Path, tool: &str, value: &T) -> Result<(), BulletinError> {
    if !valid_tool(tool) {
        return Err(BulletinError::InvalidTool(tool.to_owned()));
    }
    let body = serde_json::to_vec(value)?;
    let file = base.join(format!("{tool}.json"));
    fs::create_dir_all(base).map_err(|source| io_error(&file, source))?;
    write_atomic(&file, &body).map_err(|source| io_error(&file, source))
}

fn read_in<T: DeserializeOwned>(base: &Path, tool: &str) -> Option<T> {
    if !valid_tool(tool) {
        return None;
    }
    let bytes = fs::read(base.join(format!("{tool}.json"))).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn valid_tool(tool: &str) -> bool {
    !tool.is_empty()
        && tool
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn io_error(file: &Path, source: io::Error) -> BulletinError {
    BulletinError::Io {
        path: file.to_owned(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Status {
        version: Stamped<String>,
        gate: Stamped<bool>,
    }

    fn status(at: u64) -> Status {
        Status {
            version: Stamped {
                at,
                ttl: 60,
                value: "0.4.1".to_owned(),
            },
            gate: Stamped {
                at,
                ttl: 30,
                value: true,
            },
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn value_is_current_only_before_at_plus_ttl() {
        let stamped = Stamped {
            at: 100,
            ttl: 60,
            value: 7,
        };
        assert_eq!(stamped.current(159), Some(&7));
        assert_eq!(stamped.current(160), None);
    }

    #[test]
    fn ttl_past_the_end_of_time_stays_current() {
        let stamped = Stamped {
            at: 1,
            ttl: u64::MAX,
            value: 7,
        };
        assert_eq!(stamped.current(u64::MAX - 1), Some(&7));
    }

    #[test]
    fn written_bulletin_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        write_in(dir.path(), "acus", &status(10)).unwrap();
        assert_eq!(read_in::<Status>(dir.path(), "acus"), Some(status(10)));
    }

    #[test]
    fn write_replaces_the_previous_bulletin_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        write_in(dir.path(), "acus", &status(10)).unwrap();
        write_in(dir.path(), "acus", &status(20)).unwrap();
        assert_eq!(read_in::<Status>(dir.path(), "acus"), Some(status(20)));
        assert_eq!(names(dir.path()), ["acus.json"]);
    }

    #[test]
    fn write_creates_the_bulletin_directory() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("capsa/bulletin");
        write_in(&base, "pons", &status(10)).unwrap();
        assert_eq!(names(&base), ["pons.json"]);
    }

    #[test]
    fn missing_bulletin_reads_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_in::<Status>(dir.path(), "acus"), None);
    }

    #[test]
    fn malformed_or_mismatched_bulletin_reads_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let bodies: [&[u8]; 2] = [b"{not json", br#"{"version":1}"#];
        for body in bodies {
            fs::write(dir.path().join("acus.json"), body).unwrap();
            assert_eq!(read_in::<Status>(dir.path(), "acus"), None);
        }
    }

    #[test]
    fn unreadable_bulletin_reads_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("acus.json")).unwrap();
        assert_eq!(read_in::<Status>(dir.path(), "acus"), None);
    }

    #[test]
    fn invalid_tool_name_is_refused_without_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_in(dir.path(), "../acus", &status(10)).unwrap_err();
        assert!(matches!(err, BulletinError::InvalidTool(name) if name == "../acus"));
        assert_eq!(read_in::<Status>(dir.path(), "../acus"), None);
        assert!(names(dir.path()).is_empty());
    }
}
