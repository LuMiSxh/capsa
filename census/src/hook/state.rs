// SPDX-License-Identifier: MPL-2.0
//! Owns the hook's files: the levels shown per session and the error log.

use super::levels::Shown;
use capsa_core::paths::state_dir;
use std::io::Write;
use std::path::PathBuf;

/// The log is moved aside once it passes this size.
const LOG_LIMIT: u64 = 256 * 1024;
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
/// means another hook call holds it. The lock lasts until the returned file is dropped.
pub(super) fn lock(file: &std::path::Path) -> std::io::Result<Option<std::fs::File>> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(file.with_extension("lock"))?;
    let start = std::time::Instant::now();
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(Some(f)),
            Err(std::fs::TryLockError::WouldBlock) => {
                if start.elapsed() >= LOCK_WAIT {
                    return Ok(None);
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// Writes the levels through a temporary file and a rename, so a reader never sees half of them.
pub(super) fn store(file: &std::path::Path, shown: &Shown) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
    let mut text = shown.iter().cloned().collect::<Vec<_>>().join("\n");
    text.push('\n');
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Appends one line to `hook.log` in the state directory, moving a log over 256 KB to `hook.log.1`.
/// Failures are ignored: the hook must stay silent.
pub fn log_line(now: u64, message: &str) {
    let dir = dir();
    let file = dir.join("hook.log");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::rename(&file, dir.join("hook.log.1"));
    }
    let _ = std::fs::create_dir_all(&dir);
    let line = format!("{now} {}\n", message.replace('\n', " "));
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
    {
        let _ = f.write_all(line.as_bytes());
    }
}
