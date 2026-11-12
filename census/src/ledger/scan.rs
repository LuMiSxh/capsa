// SPDX-License-Identifier: MPL-2.0
//! Finds transcript files and reads the bytes added since the last update.

use super::model::Row;
use super::parse::parse_line;
use super::state::{FileState, Index};
use std::collections::HashSet;
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Bytes read between two looks at the deadline; the first chunk is always read.
const CHUNK: u64 = 2 << 20;

#[cfg(unix)]
fn file_id(meta: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn file_id(_meta: &Metadata) -> (u64, u64) {
    (0, 0)
}

/// Picks the files with unread bytes and drops index entries of files that are gone. A file is
/// reset to offset 0 when its identity changed or it shrank below the offset.
pub(super) fn pending(
    files: Vec<(PathBuf, Metadata)>,
    index: &mut Index,
) -> (Vec<(PathBuf, Metadata)>, bool) {
    let present: HashSet<String> = files.iter().map(|(p, _)| key(p)).collect();
    let before = index.files.len();
    index.files.retain(|k, _| present.contains(k));
    let removed = index.files.len() != before;
    let (todo, changed) = stale(files, index);
    (todo, changed || removed)
}

/// Returns the files of `files` whose bytes are not all read, and resets the state of those whose
/// identity changed or that shrank. The entries of other files stay, so one transcript can be
/// checked alone between full sweeps.
pub(super) fn stale(
    files: Vec<(PathBuf, Metadata)>,
    index: &mut Index,
) -> (Vec<(PathBuf, Metadata)>, bool) {
    let mut changed = false;
    let mut todo = Vec::new();
    for (path, meta) in files {
        let id = file_id(&meta);
        let st = index.files.entry(key(&path)).or_default();
        if (st.dev, st.ino) == id && meta.len() == st.size {
            continue;
        }
        if (st.dev, st.ino) != id || meta.len() < st.offset {
            *st = FileState {
                dev: id.0,
                ino: id.1,
                ..FileState::default()
            };
            changed = true;
        }
        todo.push((path, meta));
    }
    (todo, changed)
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Counts what one scan read.
#[derive(Default)]
pub(super) struct ScanStats {
    pub bytes: u64,
    /// The deadline stopped the scan at a line boundary before the end of the file.
    pub partial: bool,
}

/// Reads the complete new lines of a file into `out`. Rows older than `folded_before` and ids
/// already in `seen` are skipped. A trailing line without newline stays unread. After the first
/// chunk, a passed `deadline` stops the scan; the offset then sits after the last line read.
pub(super) fn scan_file(
    path: &Path,
    meta: &Metadata,
    st: &mut FileState,
    folded_before: u64,
    seen: &mut HashSet<String>,
    out: &mut Vec<Row>,
    deadline: Option<Instant>,
) -> std::io::Result<ScanStats> {
    let mut reader = BufReader::with_capacity(1 << 20, File::open(path)?);
    reader.seek(SeekFrom::Start(st.offset))?;
    let mut stats = ScanStats::default();
    let mut buf = Vec::new();
    let mut checked = 0;
    loop {
        if let Some(d) = deadline
            && stats.bytes - checked >= CHUNK
        {
            checked = stats.bytes;
            if Instant::now() >= d {
                stats.partial = true;
                break;
            }
        }
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 || buf.last() != Some(&b'\n') {
            break;
        }
        st.offset += buf.len() as u64;
        stats.bytes += buf.len() as u64;
        let Some(mut row) = std::str::from_utf8(&buf).ok().and_then(parse_line) else {
            continue;
        };
        if row.t < folded_before || !seen.insert(row.id.clone()) {
            continue;
        }
        row.gap_s = st.last_t.map(|p| row.t.saturating_sub(p));
        st.last_t = Some(row.t);
        out.push(row);
    }
    // A partial scan must not look finished to `pending`, which skips a file whose size is known.
    st.size = if stats.partial {
        st.offset
    } else {
        meta.len().max(st.offset)
    };
    Ok(stats)
}
