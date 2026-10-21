// SPDX-License-Identifier: MPL-2.0
//! Owns the guard of `~/.claude.json`, which stays writable for Claude Code. Its keys that run
//! code or grant trust on the next start are restored after the run, merged into the file as it
//! is then, so changes made by a concurrent Claude Code survive.

use anyhow::{Context, Result};
use capsa_core::hint::Hinted;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::io::{Read as _, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Lists the guarded top-level keys.
const TOP_KEYS: &[&str] = &["mcpServers"];
/// Lists the guarded keys of each `projects.<path>` entry.
const PROJECT_KEYS: &[&str] = &[
    "mcpServers",
    "allowedTools",
    "enabledMcpServers",
    "disabledMcpServers",
    "enabledMcpjsonServers",
    "disabledMcpjsonServers",
    "hasTrustDialogAccepted",
    "hasClaudeMdExternalIncludesApproved",
    "mcpContextUris",
];
/// The file is not read when it is larger than this.
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Records what reading the file found.
enum Read {
    Missing,
    Bytes(Vec<u8>),
    /// Not a regular file or too large; the text says why.
    Bad(String),
}

/// Reads `path` if it is a regular file (not a symlink, FIFO or device) of at most
/// `MAX_BYTES`. It never blocks on a FIFO and never follows a symlink.
fn read_guarded(path: &Path) -> std::io::Result<Read> {
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Read::Missing),
        Err(err) => return Err(err),
    };
    if !meta.file_type().is_file() {
        return Ok(Read::Bad("is not a regular file".to_string()));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Ok(Read::Bad("is not a regular file".to_string()));
    }
    if meta.len() > MAX_BYTES {
        return Ok(Read::Bad(format!("is larger than {MAX_BYTES} bytes")));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(Read::Bad(format!("is larger than {MAX_BYTES} bytes")));
    }
    Ok(Read::Bytes(bytes))
}

/// Holds the state of the file before the run.
#[derive(Debug)]
pub struct Guard {
    path: PathBuf,
    /// The raw pre-run bytes; `None` when the file did not exist.
    raw: Option<Vec<u8>>,
    /// The parsed pre-run document; `None` when it did not exist or did not parse.
    before: Option<Value>,
}

/// Records what `restore` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Guarded keys that were added, changed or removed, as `key` or `key (project PATH)`.
    pub changed: Vec<String>,
    /// Other problems, such as an unparsable file that was put back whole.
    pub notes: Vec<String>,
}

impl Report {
    /// Returns the warning lines for stderr; empty when nothing changed.
    pub fn warnings(&self) -> Vec<String> {
        let mut lines = self.notes.clone();
        if !self.changed.is_empty() {
            lines.push(format!(
                "warning: guarded keys in ~/.claude.json were changed during the run and have been restored: {}",
                self.changed.join(", ")
            ));
        }
        lines
    }
}

impl Guard {
    /// Reads the file now. A missing file is fine; an unreadable one, a file that is not
    /// regular and one over the size cap are errors.
    pub fn snapshot(home: &str) -> Result<Self> {
        let path = Path::new(home).join(".claude.json");
        let raw =
            match read_guarded(&path).with_context(|| format!("reading {}", path.display()))? {
                Read::Missing => None,
                Read::Bytes(raw) => Some(raw),
                Read::Bad(why) => {
                    return Err(Hinted::new(
                        format!("{} {why}", path.display()),
                        "make ~/.claude.json a regular file under 16 MiB, then run velum again",
                    )
                    .into());
                }
            };
        let before = raw
            .as_deref()
            .and_then(|raw| serde_json::from_slice(raw).ok());
        Ok(Self { path, raw, before })
    }

    /// Puts the guarded keys back. A file that is gone, not a regular file, too large or
    /// unparsable is replaced by the pre-run copy (atomically). A file that did not parse as a
    /// JSON object before the run is put back byte for byte if it differs afterwards.
    pub fn restore(&self) -> Result<Report> {
        let mut report = Report::default();
        let after_raw = match read_guarded(&self.path) {
            Ok(Read::Missing) => None,
            Ok(Read::Bytes(raw)) => Some(raw),
            Ok(Read::Bad(why)) => {
                self.put_back(&format!("{why} after the run"), &mut report)?;
                return Ok(report);
            }
            Err(err) => {
                self.put_back(
                    &format!("could not be read after the run ({err})"),
                    &mut report,
                )?;
                return Ok(report);
            }
        };
        let after: Option<Value> = after_raw
            .as_deref()
            .and_then(|raw| serde_json::from_slice(raw).ok())
            .filter(Value::is_object);
        // Fail closed: a file that was not a JSON object before cannot be merged key by key, so
        // any change to it is undone by writing the pre-run bytes back exactly. Edits made to it
        // by a concurrent Claude Code are lost then, but the file was unreadable to it anyway and
        // no byte of the original is.
        if self.baseline().is_none() {
            if after_raw != self.raw {
                self.put_back("changed during the run", &mut report)?;
            }
            return Ok(report);
        }
        let Some(mut after) = after else {
            if after_raw.is_some() {
                self.put_back(
                    "no longer parses as a JSON object after the run",
                    &mut report,
                )?;
            }
            return Ok(report);
        };
        let Some(before) = self.baseline() else {
            return Ok(report);
        };
        report.changed = restore_keys(&before, &mut after);
        if !report.changed.is_empty() {
            write_atomic(&self.path, &serde_json::to_vec_pretty(&after)?)?;
        }
        Ok(report)
    }

    /// Returns the pre-run document; an absent file counts as an empty one.
    fn baseline(&self) -> Option<Value> {
        match (&self.raw, &self.before) {
            (None, _) => Some(Value::Object(Map::new())),
            (Some(_), before) => before.clone().filter(Value::is_object),
        }
    }

    /// Replaces whatever is at the path by the pre-run copy, or removes it when there was none.
    fn put_back(&self, why: &str, report: &mut Report) -> Result<()> {
        match (&self.raw, &self.before) {
            (Some(raw), _) => {
                remove_directory(&self.path)?;
                write_atomic(&self.path, raw)?;
                report.notes.push(format!(
                    "warning: ~/.claude.json {why}; the pre-run copy was restored"
                ));
            }
            (None, _) if self.path.symlink_metadata().is_ok() => {
                remove_directory(&self.path)?;
                if self.path.symlink_metadata().is_ok() {
                    std::fs::remove_file(&self.path)
                        .with_context(|| format!("removing {}", self.path.display()))?;
                }
                report.notes.push(format!(
                    "warning: ~/.claude.json {why}; it did not exist before the run and was removed"
                ));
            }
            (None, _) => {}
        }
        Ok(())
    }
}

/// Removes `path` when it is a real directory (never through a symlink).
fn remove_directory(path: &Path) -> Result<()> {
    if path.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
        std::fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Sets every guarded key of `after` that differs from `before` back to its `before` value
/// (absent stays absent) and returns the labels of the keys that differed. Everything else
/// in `after` is left alone.
fn restore_keys(before: &Value, after: &mut Value) -> Vec<String> {
    let mut changed = Vec::new();
    for key in TOP_KEYS {
        if before.get(*key) != after.get(*key) {
            changed.push((*key).to_string());
            if let Some(root) = after.as_object_mut() {
                set_or_remove(root, key, before.get(*key));
            }
        }
    }
    let project = |doc: &Value, name: &str| doc.get("projects").and_then(|p| p.get(name)).cloned();
    let names: BTreeSet<String> = [before, &*after]
        .into_iter()
        .filter_map(|doc| doc.get("projects").and_then(Value::as_object))
        .flat_map(|map| map.keys().cloned())
        .collect();
    for name in names {
        let (old, new) = (project(before, &name), project(after, &name));
        for key in PROJECT_KEYS {
            let want = old.as_ref().and_then(|entry| entry.get(*key));
            if want == new.as_ref().and_then(|entry| entry.get(*key)) {
                continue;
            }
            changed.push(format!("{key} (project {name})"));
            if let Some(entry) = project_mut(after, &name) {
                set_or_remove(entry, key, want);
            }
        }
        // A project that did not exist before and holds nothing else is not left behind empty.
        if old.is_none()
            && let Some(projects) = after.get_mut("projects").and_then(Value::as_object_mut)
            && projects
                .get(&name)
                .is_some_and(|e| e.as_object().is_some_and(Map::is_empty))
        {
            projects.shift_remove(&name);
        }
    }
    changed.sort();
    changed
}

/// Returns `doc.projects[name]` as a mutable object, created (with `projects`) when missing.
fn project_mut<'a>(doc: &'a mut Value, name: &str) -> Option<&'a mut Map<String, Value>> {
    let projects = doc
        .as_object_mut()?
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    if !projects.is_object() {
        *projects = Value::Object(Map::new());
    }
    let entry = projects
        .as_object_mut()?
        .entry(name)
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut()
}

/// Makes `map[key]` equal `value`, or absent when `value` is `None`.
fn set_or_remove(map: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    match value {
        Some(value) => {
            map.insert(key.to_string(), value.clone());
        }
        None => {
            map.shift_remove(key);
        }
    }
}

/// Writes `bytes` to a new sibling file with mode 0600, then renames it over `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("json.velum-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = written
        .and_then(|()| std::fs::rename(&tmp, path))
        .with_context(|| format!("writing {}", path.display()));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests;
