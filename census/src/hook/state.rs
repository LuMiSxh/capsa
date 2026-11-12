// SPDX-License-Identifier: MPL-2.0
//! Owns the hook's files: the levels shown per session and the error log.

use super::levels::Shown;
use capsa_core::fs;
use capsa_core::lock::{self, Lock};
use capsa_core::log::{self, Level};
use capsa_core::paths::{log_file, state_dir};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The log is moved aside once it passes this size.
pub const LOG_LIMIT: u64 = 256 * 1024;
const ID_LIMIT: usize = 128;
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(10);

fn dir() -> PathBuf {
    state_dir("census")
}

/// Returns the file of a session's shown levels, or `None` when the id could escape the directory.
pub(super) fn hints_file(session: &str) -> Option<PathBuf> {
    let plain = !session.is_empty()
        && session.len() <= ID_LIMIT
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plain.then(|| dir().join("hints").join(session))
}

/// Reads the levels in `file`; a missing file holds none.
pub(super) fn load(file: &std::path::Path) -> std::io::Result<Shown> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(text.lines().map(str::to_owned).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Shown::new()),
        Err(e) => Err(e),
    }
}

/// Takes an exclusive lock on a sibling `.lock` file of `file`, retrying for about 10 ms. `None`
/// means another hook call holds it. The lock lasts until the returned guard is dropped.
pub(super) fn lock(file: &std::path::Path) -> std::io::Result<Option<Lock>> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    lock::exclusive(&file.with_extension("lock"), LOCK_WAIT)
}

/// Writes the levels through a temporary file and a rename, so a reader never sees half of them.
pub(super) fn store(file: &std::path::Path, shown: &Shown) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = shown.iter().cloned().collect::<Vec<_>>().join("\n");
    text.push('\n');
    fs::write_atomic(file, text.as_bytes())
}

/// The file of the hook's errors: one `hook.error` line each, in the state directory. `None` when
/// no state directory is known.
pub fn error_log() -> Option<PathBuf> {
    log_file("census", "hook.jsonl")
}

/// The fields of a `hook.error` line.
#[derive(Serialize, Deserialize)]
pub struct ErrorLine {
    pub message: String,
}

/// Appends a `hook.error` line to `error_log`. Failures are ignored: the hook must stay silent.
pub fn log_error(message: &str) {
    let Some(file) = error_log() else {
        return;
    };
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let fields = ErrorLine {
        message: message.replace('\n', " "),
    };
    let _ = log::append(&file, LOG_LIMIT, Level::Error, "hook.error", &fields);
}
