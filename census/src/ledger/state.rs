// SPDX-License-Identifier: MPL-2.0
//! Reads and writes the ledger's state files. Rewrites go through a temporary file and a rename,
//! appends are whole lines, so a reader never sees a torn file.

use super::error::LedgerError;
use super::model::{Hourly, Row};
use capsa_core::lock;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

type Result<T> = std::result::Result<T, LedgerError>;

const INDEX_VERSION: u32 = 1;

/// Names the files of one state directory.
pub(super) struct Layout {
    pub dir: PathBuf,
    pub index: PathBuf,
    pub rows: PathBuf,
    pub hourly: PathBuf,
    pub lock: PathBuf,
    pub snapshots: PathBuf,
    pub digest: PathBuf,
}

impl Layout {
    pub fn new(dir: &Path) -> Self {
        Layout {
            dir: dir.to_owned(),
            index: dir.join("index.json"),
            rows: dir.join("rows.jsonl"),
            hourly: dir.join("hourly.jsonl"),
            lock: dir.join("lock"),
            snapshots: dir.join("snapshots"),
            digest: dir.join("digest.json"),
        }
    }
}

pub(super) fn io_at(path: &Path) -> impl FnOnce(io::Error) -> LedgerError + '_ {
    move |source| LedgerError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Tracks how far one transcript file has been read.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct FileState {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub offset: u64,
    pub last_t: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Index {
    version: u32,
    /// Time of the oldest raw row, or `None` without rows.
    pub oldest_t: Option<u64>,
    /// Unix time at which the last complete sweep of all transcripts began. `None` when no sweep
    /// has completed, which includes an index written before this field existed.
    pub swept_at: Option<u64>,
    pub files: BTreeMap<String, FileState>,
}

impl Default for Index {
    fn default() -> Self {
        Index {
            version: INDEX_VERSION,
            oldest_t: None,
            swept_at: None,
            files: BTreeMap::new(),
        }
    }
}

/// Loads the index; a missing, unreadable or foreign-version file gives an empty index, which
/// makes the next update rescan everything (rows deduplicate by message id).
pub(super) fn load_index(path: &Path) -> Index {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Index>(&b).ok())
        .filter(|i| i.version == INDEX_VERSION)
        .unwrap_or_default()
}

/// Saves the index without syncs. It is a cache: a lost or torn file makes `load_index` return an
/// empty index and the next update rescans, while `rows.jsonl`, the source of truth, is appended
/// with a sync and deduplicates by message id.
pub(super) fn save_index(path: &Path, index: &Index) -> Result<()> {
    capsa_core::fs::write_cache(path, &serde_json::to_vec(index)?).map_err(io_at(path))
}

/// Replaces `path` with `bytes` through a temporary file, so a reader sees the old file or the new.
pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    capsa_core::fs::write_atomic(path, bytes).map_err(io_at(path))
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(b) => Ok(b),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(io_at(path)(e)),
    }
}

/// Yields the newline-terminated lines of `bytes`; a trailing partial line is left out.
fn complete_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let end = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    bytes[..end]
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
}

#[derive(Serialize, Deserialize)]
struct Header {
    folded_before: u64,
}

/// Loads the hourly aggregates and the time before which raw rows are folded into them.
pub(super) fn load_hourly(path: &Path) -> Result<(u64, Vec<Hourly>)> {
    let bytes = read_file(path)?;
    let mut folded_before = 0;
    let mut out = Vec::new();
    for line in complete_lines(&bytes) {
        if let Ok(h) = serde_json::from_slice::<Header>(line) {
            folded_before = h.folded_before;
        } else if let Ok(h) = serde_json::from_slice::<Hourly>(line) {
            out.push(h);
        }
    }
    Ok((folded_before, out))
}

pub(super) fn save_hourly(path: &Path, folded_before: u64, hourly: &[Hourly]) -> Result<()> {
    let mut body = serde_json::to_vec(&Header { folded_before })?;
    body.push(b'\n');
    for h in hourly {
        serde_json::to_writer(&mut body, h)?;
        body.push(b'\n');
    }
    write_atomic(path, &body)
}

/// Returns the folded-before time without parsing the aggregates.
pub(super) fn folded_before(path: &Path) -> u64 {
    let Ok(f) = File::open(path) else { return 0 };
    let mut first = Vec::new();
    let _ = io::BufRead::read_until(&mut io::BufReader::new(f), b'\n', &mut first);
    serde_json::from_slice::<Header>(&first).map_or(0, |h| h.folded_before)
}

/// Loads raw rows at or after `folded_before`, keeping the first of each message id.
pub(super) fn load_rows(path: &Path, folded_before: u64) -> Result<Vec<Row>> {
    let bytes = read_file(path)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in complete_lines(&bytes) {
        if let Ok(r) = serde_json::from_slice::<Row>(line)
            && r.t >= folded_before
            && seen.insert(r.id.clone())
        {
            out.push(r);
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
    t: u64,
}

/// Loads the message ids of raw rows at or after `folded_before`.
pub(super) fn load_ids(path: &Path, folded_before: u64) -> Result<HashSet<String>> {
    let bytes = read_file(path)?;
    Ok(complete_lines(&bytes)
        .filter_map(|l| serde_json::from_slice::<IdOnly>(l).ok())
        .filter(|r| r.t >= folded_before)
        .map(|r| r.id)
        .collect())
}

/// Cuts a trailing line without newline, left by a writer killed mid-write, so the next append
/// does not glue onto it. The caller holds the writer lock.
fn drop_partial_tail(f: &mut File) -> io::Result<()> {
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    let mut end = len;
    let mut block = vec![0_u8; 64 * 1024];
    while end > 0 {
        let start = end.saturating_sub(block.len() as u64);
        let n = usize::try_from(end - start).unwrap_or(block.len());
        f.seek(SeekFrom::Start(start))?;
        f.read_exact(&mut block[..n])?;
        if let Some(i) = block[..n].iter().rposition(|&b| b == b'\n') {
            let keep = start + i as u64 + 1;
            if keep < len {
                f.set_len(keep)?;
            }
            return Ok(());
        }
        end = start;
    }
    f.set_len(0)
}

/// Appends rows as whole lines in one write and flushes them. A torn last line is cut first.
pub(super) fn append_rows(path: &Path, rows: &[Row]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut body = Vec::with_capacity(rows.len() * 256);
    for r in rows {
        serde_json::to_writer(&mut body, r)?;
        body.push(b'\n');
    }
    let mut f = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(io_at(path))?;
    drop_partial_tail(&mut f).map_err(io_at(path))?;
    f.seek(SeekFrom::End(0)).map_err(io_at(path))?;
    f.write_all(&body).map_err(io_at(path))?;
    f.sync_data().map_err(io_at(path))
}

pub(super) fn rewrite_rows(path: &Path, rows: &[Row]) -> Result<()> {
    let mut body = Vec::with_capacity(rows.len() * 256);
    for r in rows {
        serde_json::to_writer(&mut body, r)?;
        body.push(b'\n');
    }
    write_atomic(path, &body)
}

/// Takes the shared lock, retrying until `wait` has passed. `None` means a writer holds it or the
/// lock file cannot be opened; the caller then reads without it.
pub(super) fn lock_shared(path: &Path, wait: Duration) -> Option<lock::Lock> {
    lock::shared(path, wait).ok().flatten()
}

/// Takes the lock, retrying until `wait` has passed; `None` means another process holds it.
pub(super) fn try_lock(path: &Path, wait: Duration) -> Result<Option<lock::Lock>> {
    lock::exclusive(path, wait).map_err(io_at(path))
}
