// SPDX-License-Identifier: MPL-2.0
//! Checks a JSON-lines log a tool writes: that it takes lines, how close it is to its rotation
//! limit and how old its last entry is.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::check;
use crate::format::ago;
use crate::render::{Check, Status};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
/// The first window read from the end of the log. A last entry longer than this reads a wider one.
const TAIL_BYTES: u64 = 4096;

/// The one envelope field the age check reads.
#[derive(Deserialize)]
struct Stamp {
    ts: u64,
}

/// Builds the three health checks of the log at `path`: that it takes lines, its size against
/// `max_bytes`, the rotation limit, and the age of its last entry.
///
/// `label` names the log in the check names: `"guard"` gives `guard log file`, `guard log size` and
/// `last guard entry`, and it names the writer in the hint of an oversized log.
pub fn log_checks(label: &str, path: &Path, max_bytes: u64) -> Vec<Check> {
    vec![
        log_file_check(label, path),
        log_size_check(label, path, max_bytes),
        log_age_check(label, path, now_secs()),
    ]
}

/// Reports whether the log at `path` takes lines. A log that does not exist yet is not an error.
pub(super) fn log_file_check(label: &str, path: &Path) -> Check {
    let name = format!("{label} log file");
    match OpenOptions::new().append(true).open(path) {
        Ok(_) => check(name, Status::Ok, "writable", None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            check(name, Status::Ok, "no log yet", None)
        }
        Err(e) => check(
            name,
            Status::Fail,
            format!("{} not writable: {e}", path.display()),
            Some("make the log writable, or delete it so the next entry creates a new one"),
        ),
    }
}

/// Reports the size of the log at `path` against `max_bytes`, warning once it is past the limit.
pub(super) fn log_size_check(label: &str, path: &Path, max_bytes: u64) -> Check {
    let name = format!("{label} log size");
    let Ok(meta) = fs::metadata(path) else {
        return check(name, Status::Ok, "no log yet", None);
    };
    let size = format!("{} KiB", meta.len() / KIB);
    if meta.len() <= max_bytes {
        return check(name, Status::Ok, size, None);
    }
    let hint = format!(
        "the log should rotate at {}; check the {label} runs",
        size_text(max_bytes)
    );
    check(name, Status::Warn, size, Some(hint.as_str()))
}

/// Reports the age of the last entry of the log at `path`, given the current time `now` in unix
/// seconds. An entry stamped after `now` is `just now`.
pub(super) fn log_age_check(label: &str, path: &Path, now: u64) -> Check {
    let name = format!("last {label} entry");
    match last_ts(path) {
        Some(ts) => check(name, Status::Ok, ago(now.saturating_sub(ts)), None),
        None => check(name, Status::Ok, "no entries yet", None),
    }
}

/// Returns the `ts` of the last complete line of the file at `path`, reading only its tail.
///
/// The window grows from [`TAIL_BYTES`] until it holds a whole line or reaches the start of the
/// file. A torn or unparsable last line yields `None`, as no entry.
pub(super) fn last_ts(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut window = TAIL_BYTES;
    loop {
        let start = len.saturating_sub(window);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut tail = Vec::new();
        file.read_to_end(&mut tail).ok()?;
        let mut lines = tail.split(|&b| b == b'\n');
        if start > 0 {
            // The first piece starts mid-line, so it is not an entry.
            lines.next();
        }
        match lines.rev().find(|line| !line.trim_ascii().is_empty()) {
            Some(line) => return serde_json::from_slice::<Stamp>(line).ok().map(|s| s.ts),
            None if start == 0 => return None,
            None => window = window.saturating_mul(2),
        }
    }
}

/// States a byte count in the largest of `MiB` or `KiB` that divides it exactly, else in bytes.
pub(super) fn size_text(bytes: u64) -> String {
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else if bytes >= KIB && bytes.is_multiple_of(KIB) {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Returns the current time in whole seconds since the Unix epoch.
pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
