// SPDX-License-Identifier: MPL-2.0
//! Owns the file of rate-limit snapshots that census and pons share: one JSON object per line.

use crate::fs::write_atomic;
use crate::paths::state_dir;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Names where a snapshot was observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Statusline,
    Pons,
}

/// Names the rolling limit window a snapshot describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    FiveHour,
    SevenDay,
}

/// Holds one observation of a subscription limit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Unix seconds of the observation.
    pub t: u64,
    pub source: Source,
    pub window: Window,
    /// Share of the window used, 0 to 100.
    pub used_percentage: f64,
    /// Unix seconds at which the window resets.
    pub resets_at: u64,
}

/// Lists what can go wrong when reading or writing the snapshot file.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("cannot access {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot encode the snapshot")]
    Encode(#[from] serde_json::Error),
}

fn io_error(file: &Path, source: io::Error) -> SnapshotError {
    SnapshotError::Io {
        path: file.to_owned(),
        source,
    }
}

/// Returns the default snapshot file, `snapshots` in the census state directory.
pub fn path() -> PathBuf {
    state_dir("census").join("snapshots")
}

/// Appends a snapshot to the default file, creating the directories.
///
/// # Errors
/// Fails when the line cannot be encoded or written.
pub fn append(snapshot: &Snapshot) -> Result<(), SnapshotError> {
    append_to(&path(), snapshot)
}

/// Appends a snapshot to `file` with one `O_APPEND` write, which keeps concurrent writers from
/// interleaving because a line stays far below the atomic write size.
///
/// # Errors
/// Fails when the line cannot be encoded or written.
pub fn append_to(file: &Path, snapshot: &Snapshot) -> Result<(), SnapshotError> {
    let mut line = serde_json::to_vec(snapshot)?;
    line.push(b'\n');
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|source| io_error(file, source))?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .map_err(|source| io_error(file, source))?;
    f.write_all(&line).map_err(|source| io_error(file, source))
}

/// Reads the snapshots with `t >= since` from the default file.
///
/// # Errors
/// Fails when the file exists but cannot be read.
pub fn read_since(since: u64) -> Result<Vec<Snapshot>, SnapshotError> {
    read_since_from(&path(), since)
}

/// Reads the snapshots with `t >= since` from `file`, skipping lines that do not parse. A missing
/// file gives an empty list.
///
/// # Errors
/// Fails when the file exists but cannot be read.
pub fn read_since_from(file: &Path, since: u64) -> Result<Vec<Snapshot>, SnapshotError> {
    let f = match File::open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_error(file, e)),
    };
    let mut out = Vec::new();
    for line in BufReader::new(f).split(b'\n') {
        let line = line.map_err(|source| io_error(file, source))?;
        if let Ok(s) = serde_json::from_slice::<Snapshot>(&line)
            && s.t >= since
        {
            out.push(s);
        }
    }
    Ok(out)
}

/// Counts, and unless `dry_run` removes, the snapshots older than `before` in `file`. The file is
/// replaced through a temporary file, so a concurrent append between the read and the rename can
/// be lost.
///
/// # Errors
/// Fails when the file cannot be read or replaced.
pub fn prune_before(file: &Path, before: u64, dry_run: bool) -> Result<usize, SnapshotError> {
    let all = read_since_from(file, 0)?;
    let (keep, drop): (Vec<_>, Vec<_>) = all.into_iter().partition(|s| s.t >= before);
    if dry_run || drop.is_empty() {
        return Ok(drop.len());
    }
    let mut body = Vec::new();
    for s in &keep {
        serde_json::to_writer(&mut body, s)?;
        body.push(b'\n');
    }
    write_atomic(file, &body).map_err(|source| io_error(file, source))?;
    Ok(drop.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(t: u64) -> Snapshot {
        Snapshot {
            t,
            source: Source::Pons,
            window: Window::FiveHour,
            used_percentage: 12.5,
            resets_at: t + 100,
        }
    }

    #[test]
    fn appended_snapshots_are_read_back_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deep/snapshots");
        append_to(&file, &snap(10)).unwrap();
        append_to(&file, &snap(20)).unwrap();
        assert_eq!(read_since_from(&file, 15).unwrap(), vec![snap(20)]);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("snapshots");
        append_to(&file, &snap(10)).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(b"{not json\n")
            .unwrap();
        append_to(&file, &snap(30)).unwrap();
        assert_eq!(read_since_from(&file, 0).unwrap().len(), 2);
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            read_since_from(&dir.path().join("none"), 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn line_uses_documented_field_names_and_stays_small() {
        let line = serde_json::to_string(&snap(1_800_000_000)).unwrap();
        assert!(line.contains(r#""source":"pons""#) && line.contains(r#""window":"five_hour""#));
        assert!(line.len() < 512);
    }

    #[test]
    fn prune_dry_run_counts_without_removing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("snapshots");
        for t in [10, 20, 30] {
            append_to(&file, &snap(t)).unwrap();
        }
        assert_eq!(prune_before(&file, 25, true).unwrap(), 2);
        assert_eq!(read_since_from(&file, 0).unwrap().len(), 3);
        assert_eq!(prune_before(&file, 25, false).unwrap(), 2);
        assert_eq!(read_since_from(&file, 0).unwrap(), vec![snap(30)]);
    }
}
