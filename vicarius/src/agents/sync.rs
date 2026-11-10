// SPDX-License-Identifier: MPL-2.0
//! Writes, refreshes and removes the derived agent files in the agents directory.

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;

use super::frontmatter::{derive, is_derived};
use crate::config::Agent;

const SUFFIX: &str = "-ds.md";

/// What one sync did, with an entry for each derived file it handled.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SyncReport {
    /// The entries in the order the files were handled.
    pub entries: Vec<Entry>,
}

/// The outcome for one derived file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// The agent the file belongs to, taken from its name.
    pub agent: String,
    /// The derived file's name, such as `worker-ds.md`.
    pub file: String,
    /// What the sync did with the file.
    pub outcome: Outcome,
}

/// What a sync did with one derived file, or why it left the file alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The file was written, or rewritten after its source changed.
    Written,
    /// The file already holds the derived content.
    Unchanged,
    /// The file was removed: its agent is no longer configured, DeepSeek is unavailable or its
    /// source is gone.
    Removed,
    /// The source agent file is missing and no derived file is left behind.
    SourceMissing,
    /// The source cannot be derived; the text is the error.
    Unparsable(String),
    /// A file without the vicarius marker holds the name, so it is left alone.
    Conflict,
    /// Reading or writing failed; the text is the error.
    Error(String),
}

/// Brings the derived agent files in `dir` in step with `agents` and `available`.
///
/// When `available`, each agent gets a `<from>-ds.md` written from `<from>.md`, and derived files of
/// agents that are no longer configured are removed. When not, every derived file is removed.
/// A suspended agent is treated as not configured, so its derived file is removed. A derived file
/// whose source is missing is removed. Hand-written files are never written or removed. The
/// directory is created only when a file is written.
pub fn sync(dir: &Path, agents: &[Agent], available: bool) -> SyncReport {
    let mut report = SyncReport::default();
    if available {
        report.entries.extend(
            agents
                .iter()
                .filter(|agent| !agent.suspended)
                .map(|agent| sync_agent(dir, agent)),
        );
        remove_derived(dir, agents, &mut report);
    } else {
        remove_derived(dir, &[], &mut report);
    }
    report
}

fn sync_agent(dir: &Path, agent: &Agent) -> Entry {
    let file = format!("{}{SUFFIX}", agent.from);
    let outcome = write_agent(dir, agent, &file);
    Entry {
        agent: agent.from.clone(),
        file,
        outcome,
    }
}

fn write_agent(dir: &Path, agent: &Agent, file: &str) -> Outcome {
    let source = match fs::read_to_string(dir.join(format!("{}.md", agent.from))) {
        Ok(source) => source,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return remove_orphan(&dir.join(file)),
        Err(e) => return Outcome::Error(e.to_string()),
    };
    let derived = match derive(&source, &agent.from, &agent.model, agent.effort) {
        Ok(derived) => derived,
        Err(e) => return Outcome::Unparsable(e.to_string()),
    };
    match read_lossy(&dir.join(file)) {
        Ok(existing) if !is_derived(&existing) => return Outcome::Conflict,
        Ok(existing) if existing == derived => return Outcome::Unchanged,
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Outcome::Error(e.to_string()),
    }
    match write_atomic(dir, file, &derived) {
        Ok(()) => Outcome::Written,
        Err(e) => Outcome::Error(e.to_string()),
    }
}

/// Removes `path` when it is a derived file whose source is gone, else reports the missing source.
fn remove_orphan(path: &Path) -> Outcome {
    match read_lossy(path) {
        Ok(text) if is_derived(&text) => match fs::remove_file(path) {
            Ok(()) => Outcome::Removed,
            Err(e) => Outcome::Error(e.to_string()),
        },
        _ => Outcome::SourceMissing,
    }
}

fn write_atomic(dir: &Path, file: &str, content: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".{file}.tmp"));
    let result = fs::write(&temp, content).and_then(|()| fs::rename(&temp, dir.join(file)));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn read_lossy(path: &Path) -> io::Result<String> {
    fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn remove_derived(dir: &Path, agents: &[Agent], report: &mut SyncReport) {
    let files = match derived_files(dir) {
        Ok(files) => files,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            report.entries.push(Entry {
                agent: String::new(),
                file: dir.display().to_string(),
                outcome: Outcome::Error(e.to_string()),
            });
            return;
        }
    };
    for (agent, file) in files {
        if agents
            .iter()
            .any(|configured| !configured.suspended && configured.from == agent)
        {
            continue;
        }
        let path = dir.join(&file);
        let outcome = match read_lossy(&path) {
            Ok(text) if !is_derived(&text) => continue,
            Ok(_) => match fs::remove_file(&path) {
                Ok(()) => Outcome::Removed,
                Err(e) => Outcome::Error(e.to_string()),
            },
            Err(e) => Outcome::Error(e.to_string()),
        };
        report.entries.push(Entry {
            agent,
            file,
            outcome,
        });
    }
}

/// Lists the `<agent>-ds.md` files in `dir` as `(agent, file)` pairs, sorted by file name.
fn derived_files(dir: &Path) -> io::Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let file = entry?.file_name().to_string_lossy().into_owned();
        let agent = match file.strip_suffix(SUFFIX) {
            Some(agent) if !agent.is_empty() => agent.to_owned(),
            _ => continue,
        };
        files.push((agent, file));
    }
    files.sort();
    Ok(files)
}
