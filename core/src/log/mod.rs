// SPDX-License-Identifier: MPL-2.0
//! Owns the JSON-lines logs the capsa tools write: one object per line, with a shared envelope.

#[cfg(test)]
mod tests;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Bytes buffered by a `Writer` before it flushes on its own.
const BUFFER_LIMIT: usize = 8 * 1024;

/// Names how serious a logged event is; `Warn` and `Error` reach the file at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Serialize)]
struct Line<'a, T: ?Sized> {
    ts: u64,
    level: Level,
    event: &'a str,
    #[serde(flatten)]
    fields: &'a T,
}

/// Holds one parsed line: the envelope, then the tool's own fields.
#[derive(Debug, Deserialize)]
pub struct Entry<T> {
    pub ts: u64,
    /// Absent in lines written before the envelope carried a level.
    pub level: Option<Level>,
    /// Absent in lines written before the envelope carried an event name.
    pub event: Option<String>,
    #[serde(flatten)]
    pub fields: T,
}

/// Appends one line, first renaming `path` to `rotated(path)` when it exceeds `max_bytes`.
///
/// # Errors
/// Fails when the line cannot be encoded, rotated or written.
pub fn append<T: Serialize + ?Sized>(
    path: &Path,
    max_bytes: u64,
    level: Level,
    event: &str,
    fields: &T,
) -> io::Result<()> {
    let line = encode(level, event, fields)?;
    if std::fs::metadata(path).is_ok_and(|m| m.len() > max_bytes) {
        rotate_file(path)?;
    }
    open_append(path)?.write_all(line.as_bytes())
}

/// Returns the file holding the previous generation of `path`: `<file name>.old`.
pub fn rotated(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

/// Reads the lines of `rotated(path)` and then of `path`, skipping any that do not parse.
pub fn read<T: DeserializeOwned>(path: &Path) -> Vec<Entry<T>> {
    let mut entries = Vec::new();
    for file in [rotated(path), path.to_owned()] {
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        entries.extend(
            bytes
                .split(|&b| b == b'\n')
                .filter_map(|line| serde_json::from_slice::<Entry<T>>(line).ok()),
        );
    }
    entries
}

/// Keeps one log file open across writes, tracks its size instead of asking the file system per
/// line, and writes out its buffer on `Warn`/`Error`, on `flush` and on drop.
pub struct Writer {
    path: PathBuf,
    max_bytes: u64,
    file: Option<File>,
    /// Bytes in the file plus bytes still in `buf`.
    size: u64,
    buf: Vec<u8>,
}

impl Writer {
    /// Opens `path` for appending, creating its directory when it is missing.
    ///
    /// # Errors
    /// Fails when the file cannot be opened or its size cannot be read.
    pub fn open(path: &Path, max_bytes: u64) -> io::Result<Self> {
        let file = open_append(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path: path.to_owned(),
            max_bytes,
            file: Some(file),
            size,
            buf: Vec::with_capacity(BUFFER_LIMIT),
        })
    }

    /// Buffers one line, rotating first when the file exceeds `max_bytes`.
    ///
    /// # Errors
    /// Fails when the line cannot be encoded, or when a rotation or a flush fails.
    pub fn write<T: Serialize + ?Sized>(
        &mut self,
        level: Level,
        event: &str,
        fields: &T,
    ) -> io::Result<()> {
        let line = encode(level, event, fields)?;
        if self.size > self.max_bytes {
            self.rotate()?;
        }
        self.buf.extend_from_slice(line.as_bytes());
        self.size += line.len() as u64;
        if matches!(level, Level::Warn | Level::Error) || self.buf.len() >= BUFFER_LIMIT {
            self.flush()?;
        }
        Ok(())
    }

    /// Writes the buffered lines to the file.
    ///
    /// # Errors
    /// Fails when the file cannot be opened or written. The buffer is cleared either way.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let mut file = match self.file.take() {
            Some(file) => file,
            None => open_append(&self.path)?,
        };
        let result = file.write_all(&self.buf);
        self.file = Some(file);
        self.buf.clear();
        result
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.flush()?;
        // The handle must close before the rename: Windows refuses to move an open file.
        self.file = None;
        rotate_file(&self.path)?;
        self.size = 0;
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

fn encode<T: Serialize + ?Sized>(level: Level, event: &str, fields: &T) -> io::Result<String> {
    let line = Line {
        ts: now_secs(),
        level,
        event,
        fields,
    };
    let mut text = serde_json::to_string(&line).map_err(io::Error::other)?;
    text.push('\n');
    Ok(text)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Opens `path` for appending; the directory is created only when the first open finds none.
fn open_append(path: &Path) -> io::Result<File> {
    let open = || OpenOptions::new().create(true).append(true).open(path);
    match open() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            open()
        }
        other => other,
    }
}

/// Renames `path` to its rotated name, replacing the previous generation. A file another
/// process has already rotated is not an error.
fn rotate_file(path: &Path) -> io::Result<()> {
    match std::fs::rename(path, rotated(path)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}
