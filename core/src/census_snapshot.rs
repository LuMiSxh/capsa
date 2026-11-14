// SPDX-License-Identifier: MPL-2.0
//! Owns the file of rate-limit snapshots that census and pons share: one JSON object per line.

use crate::fs::write_atomic;
use crate::paths::state_dir;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
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

/// Reads the snapshots with `t >= since` from `file` in file order, skipping lines that do not
/// parse. A missing file gives an empty list.
///
/// The file is scanned from its end and the scan stops at the first snapshot older than
/// `since - ORDER_SLACK`, so the result matches a whole-file scan as long as no line trails a newer
/// line by more than `ORDER_SLACK` seconds. A longer disorder can hide a snapshot that sits before
/// an older one.
///
/// # Errors
/// Fails when the file exists but cannot be read.
pub fn read_since_from(file: &Path, since: u64) -> Result<Vec<Snapshot>, SnapshotError> {
    read_tail(file, since, TAIL_CHUNK)
}

/// Bytes read per step of the backward scan. A snapshot line is under 512 bytes, so one step
/// holds hundreds of lines.
const TAIL_CHUNK: u64 = 64 * 1024;

/// Seconds by which a line may trail a newer line before the backward scan can miss it. Census and
/// pons append independently, so lines can be out of order by a short time.
const ORDER_SLACK: u64 = 300;

/// Reads like [`read_since_from`], taking `chunk` bytes per backward step.
fn read_tail(file: &Path, since: u64, chunk: u64) -> Result<Vec<Snapshot>, SnapshotError> {
    let mut f = match File::open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_error(file, e)),
    };
    let len = f.metadata().map_err(|e| io_error(file, e))?.len();
    let mut newest_first = Vec::new();
    let floor = since.saturating_sub(ORDER_SLACK);
    // ponytail: a line longer than one chunk is copied again at every step, so a long line costs
    // quadratic time. Snapshot lines are under 512 bytes; a reverse line reader replaces the copy
    // if the format grows.
    let mut carry: Vec<u8> = Vec::new();
    let mut end = len;
    'scan: loop {
        let start = end.saturating_sub(chunk);
        let mut buf = read_range(&mut f, start, end).map_err(|e| io_error(file, e))?;
        buf.extend_from_slice(&carry);
        let mut lines = buf.split(|&b| b == b'\n');
        let head = lines.next().unwrap_or_default();
        for line in lines.rev() {
            if !push_since(line, since, floor, &mut newest_first) {
                break 'scan;
            }
        }
        if start == 0 {
            push_since(head, since, floor, &mut newest_first);
            break;
        }
        // The head may start inside a line that the next, earlier chunk completes.
        carry = head.to_vec();
        end = start;
    }
    newest_first.reverse();
    Ok(newest_first)
}

/// Pushes the snapshot in `line` onto `newest_first` when it is at or after `since`. Returns
/// `false` at a snapshot before `floor`, which ends the backward scan.
fn push_since(line: &[u8], since: u64, floor: u64, newest_first: &mut Vec<Snapshot>) -> bool {
    match serde_json::from_slice::<Snapshot>(line) {
        Ok(s) if s.t < floor => false,
        Ok(s) => {
            if s.t >= since {
                newest_first.push(s);
            }
            true
        }
        Err(_) => true,
    }
}

/// Reads the bytes `start..end` of `file`.
fn read_range(file: &mut File, start: u64, end: u64) -> io::Result<Vec<u8>> {
    let mut buf = vec![0; usize::try_from(end - start).map_err(io::Error::other)?];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut buf)?;
    Ok(buf)
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

    /// Returns the snapshot lines for `times`, each ending with a newline.
    fn lines(times: &[u64]) -> Vec<u8> {
        let mut out = Vec::new();
        for &t in times {
            serde_json::to_writer(&mut out, &snap(t)).unwrap();
            out.push(b'\n');
        }
        out
    }

    /// Returns what a whole-file scan reads, the reference for every tail read.
    fn full_scan(file: &Path, since: u64) -> Vec<Snapshot> {
        let Ok(bytes) = std::fs::read(file) else {
            return Vec::new();
        };
        bytes
            .split(|&b| b == b'\n')
            .filter_map(|line| serde_json::from_slice::<Snapshot>(line).ok())
            .filter(|s| s.t >= since)
            .collect()
    }

    /// Checks the tail reads against the full scan for `content`, at every chunk size and `since`.
    fn assert_matches_full_scan(content: &[u8]) {
        const SINCE: [u64; 13] = [
            0,
            1,
            10,
            15,
            20,
            45,
            50,
            800,
            1000,
            1100,
            2990,
            3000,
            u64::MAX,
        ];
        const CHUNKS: [u64; 8] = [1, 2, 3, 7, 64, 100, 257, TAIL_CHUNK];
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("snapshots");
        std::fs::write(&file, content).unwrap();
        for since in SINCE {
            let want = full_scan(&file, since);
            assert_eq!(
                read_since_from(&file, since).unwrap(),
                want,
                "since {since}"
            );
            for chunk in CHUNKS {
                let got = read_tail(&file, since, chunk).unwrap();
                assert_eq!(got, want, "since {since}, chunk {chunk}");
            }
        }
    }

    #[test]
    fn tail_read_matches_full_scan_at_every_chunk_size() {
        let mut malformed = lines(&[10, 20]);
        malformed.extend_from_slice(b"{not json\n\n");
        malformed.extend_from_slice(&lines(&[30]));
        let mut no_newline = lines(&[10, 20]);
        no_newline.pop();
        let long: Vec<u64> = (0..1000).map(|i| i * 10).collect();
        let contents = [
            vec![],
            lines(&[10]),
            lines(&[10, 20, 30, 40]),
            malformed,
            no_newline,
            lines(&long),
        ];
        for content in contents {
            assert_matches_full_scan(&content);
        }
    }

    #[test]
    fn partial_last_line_is_skipped_like_a_full_scan() {
        let mut partial = lines(&[10, 20]);
        partial.extend_from_slice(br#"{"t":30,"source":"pons""#);
        assert_matches_full_scan(&partial);
    }

    #[test]
    fn file_smaller_than_the_chunk_reads_like_a_full_scan() {
        assert_matches_full_scan(&lines(&[10, 20, 30]));
    }

    #[test]
    fn disorder_within_the_slack_matches_full_scan() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("snapshots");
        std::fs::write(&file, lines(&[100, 5, 200])).unwrap();
        assert_eq!(read_since_from(&file, 50).unwrap(), full_scan(&file, 50));
        assert_eq!(
            read_since_from(&file, 50).unwrap(),
            vec![snap(100), snap(200)]
        );
        assert_matches_full_scan(&lines(&[1000, 800, 1200]));
    }

    #[test]
    fn disorder_beyond_the_slack_is_not_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("snapshots");
        std::fs::write(&file, lines(&[1100, 100, 1200])).unwrap();
        assert_eq!(full_scan(&file, 1000), vec![snap(1100), snap(1200)]);
        assert_eq!(read_since_from(&file, 1000).unwrap(), vec![snap(1200)]);
    }
}
