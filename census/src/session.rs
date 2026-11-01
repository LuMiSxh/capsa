// SPDX-License-Identifier: MPL-2.0
//! Owns the Claude Code session that `census context` and `census session` describe: which
//! transcript it is, and what it has done so far.

use crate::context::{self, ContextUse};
use crate::transcript::{self, Record};
use crate::usage::{Cost, Tokens, parse_ts};
use capsa_core::hint::Hint;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How a session was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Picked {
    /// The `--session` flag.
    Flag,
    /// The `CLAUDE_SESSION_ID` environment variable.
    Env,
    /// The most recently modified transcript whose directory is the current one, an ancestor or a
    /// descendant of it.
    Recent,
}

impl Picked {
    /// The label that names how the session was chosen.
    pub fn label(self) -> &'static str {
        match self {
            Self::Flag => "--session",
            Self::Env => "CLAUDE_SESSION_ID",
            Self::Recent => "most recent transcript for this directory",
        }
    }
}

/// A session and the main transcript that records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub id: String,
    pub transcript: PathBuf,
    pub picked: Picked,
}

/// Lists what can go wrong when choosing a session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("no transcript for session {0}")]
    UnknownId(String),
    #[error("no Claude Code session was recorded in {}", .0.display())]
    NoSession(PathBuf),
    #[error("cannot read the current directory")]
    Cwd(#[source] std::io::Error),
}

impl Hint for SessionError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::UnknownId(_) => Some("pass the id of a session under ~/.claude/projects".into()),
            Self::NoSession(_) => {
                Some("run it in a directory where Claude Code ran, or pass --session ID".into())
            }
            Self::Cwd(_) => None,
        }
    }
}

/// Chooses the session of this process: `--session`, else `CLAUDE_SESSION_ID`, else the most
/// recent transcript for the current directory.
///
/// # Errors
/// Fails when the current directory is unknown, the named session has no transcript, or no
/// transcript matches the current directory.
pub fn resolve_here(flag: Option<&str>) -> Result<Resolved, SessionError> {
    let cwd = std::env::current_dir().map_err(SessionError::Cwd)?;
    let roots = [context::claude_config_dir().join("projects")];
    let env = std::env::var("CLAUDE_SESSION_ID").ok();
    resolve(&roots, &cwd, flag, env.as_deref())
}

/// Chooses the session among the transcripts under `roots`: `flag`, else `env`, else the most
/// recently modified main transcript whose recorded directory is `cwd`, an ancestor or a descendant
/// of it. Subagent transcripts are never chosen.
///
/// # Errors
/// Fails when a named session has no transcript, or no transcript matches `cwd`.
pub fn resolve(
    roots: &[PathBuf],
    cwd: &Path,
    flag: Option<&str>,
    env: Option<&str>,
) -> Result<Resolved, SessionError> {
    if let Some(id) = flag {
        return by_id(roots, id, Picked::Flag);
    }
    match env.filter(|id| !id.is_empty()) {
        Some(id) => by_id(roots, id, Picked::Env),
        None => recent(roots, cwd),
    }
}

fn by_id(roots: &[PathBuf], id: &str, picked: Picked) -> Result<Resolved, SessionError> {
    main_transcripts(roots)
        .into_iter()
        .find(|p| p.file_stem().is_some_and(|stem| stem == id))
        .map(|transcript| Resolved {
            id: id.to_owned(),
            transcript,
            picked,
        })
        .ok_or_else(|| SessionError::UnknownId(id.to_owned()))
}

fn recent(roots: &[PathBuf], cwd: &Path) -> Result<Resolved, SessionError> {
    let mut files: Vec<(SystemTime, PathBuf)> = main_transcripts(roots)
        .into_iter()
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
        .into_iter()
        .map(|(_, transcript)| transcript)
        .find(|p| transcript::cwd_of(p).is_some_and(|c| related(cwd, Path::new(&c))))
        .and_then(|transcript| {
            let id = transcript.file_stem()?.to_string_lossy().into_owned();
            Some(Resolved {
                id,
                transcript,
                picked: Picked::Recent,
            })
        })
        .ok_or_else(|| SessionError::NoSession(cwd.to_owned()))
}

/// Whether `recorded` is `cwd`, an ancestor of it or a descendant of it, compared by real path so
/// that one directory under two spellings (a short Windows name, a symlink) still matches.
fn related(cwd: &Path, recorded: &Path) -> bool {
    let (cwd, recorded) = (real(cwd), real(recorded));
    cwd.starts_with(&recorded) || recorded.starts_with(&cwd)
}

/// Returns the real path of `path`, or `path` itself when it does not exist.
fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The main transcripts under the roots: `<root>/<project>/<session>.jsonl`.
fn main_transcripts(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter_map(|root| std::fs::read_dir(root).ok())
        .flatten()
        .flatten()
        .map(|project| project.path())
        .filter(|p| p.is_dir())
        .filter_map(|project| std::fs::read_dir(project).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "jsonl"))
        .collect()
}

/// What a session has done, from its main transcript and its subagents'.
#[derive(Debug, Serialize)]
pub struct Summary {
    pub cwd: Option<String>,
    /// The model of the session's last request.
    pub model: Option<String>,
    /// Seconds from the first record to the last one.
    pub duration_s: u64,
    /// API requests, one per message id, main and subagents together.
    pub requests: u64,
    pub subagents: u64,
    pub cost: Cost,
    /// Subagents per role, from `attributionAgent`.
    pub spawns: BTreeMap<String, u64>,
    pub compactions: u64,
    /// The context at the session's last request, against the threshold.
    pub context: Option<ContextUse>,
}

/// Summarizes the session whose main transcript is `main`; its subagent transcripts are the
/// `.jsonl` files in the `subagents` directory beside it.
///
/// # Errors
/// Fails when the main transcript cannot be read.
pub fn summarize(main: &Path) -> std::io::Result<Summary> {
    let main_scan = Scan::of(&std::fs::read_to_string(main)?);
    let subagents: Vec<(String, Scan)> = subagent_files(main)
        .into_iter()
        .filter_map(|p| {
            let scan = Scan::of(&std::fs::read_to_string(p).ok()?);
            let role = scan.role.clone().unwrap_or_else(|| "unknown".to_owned());
            Some((role, scan))
        })
        .collect();
    let scans = || std::iter::once(&main_scan).chain(subagents.iter().map(|(_, s)| s));

    let mut cost = Cost::default();
    let mut requests = 0;
    for scan in scans() {
        for (model, tokens) in &scan.requests {
            cost.add_request(model, tokens);
            requests += 1;
        }
    }
    let first = scans().filter_map(|s| s.first_ms).min();
    let last = scans().filter_map(|s| s.last_ms).max();
    let duration_s = first
        .zip(last)
        .map_or(0, |(a, b)| b.saturating_sub(a) / 1000);
    let mut spawns = BTreeMap::new();
    for (role, _) in &subagents {
        *spawns.entry(role.clone()).or_insert(0) += 1;
    }
    let last_request = main_scan.requests.last();
    Ok(Summary {
        cwd: transcript::cwd_of(main),
        model: last_request.map(|(model, _)| model.clone()),
        duration_s,
        requests,
        subagents: subagents.len() as u64,
        cost,
        spawns,
        compactions: scans().map(|s| s.compactions).sum(),
        context: last_request
            .map(|(model, tokens)| context::measure(tokens.context(), Some(model))),
    })
}

/// The subagent transcripts beside a main transcript, sorted by name.
fn subagent_files(main: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(main.with_extension("").join("subagents")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort();
    files
}

/// What one transcript file holds, in file order.
#[derive(Default)]
struct Scan {
    /// Model and tokens of each request, each message id once.
    requests: Vec<(String, Tokens)>,
    compactions: u64,
    first_ms: Option<u64>,
    last_ms: Option<u64>,
    /// The `attributionAgent` of the file's records, when it has one.
    role: Option<String>,
}

impl Scan {
    fn of(text: &str) -> Self {
        let mut scan = Scan::default();
        let mut seen = HashSet::new();
        for record in transcript::records(text) {
            scan.note_time(&record);
            if scan.role.is_none() {
                scan.role.clone_from(&record.attribution_agent);
            }
            if is_compaction(&record) {
                scan.compactions += 1;
            }
            if let Some(req) = record.request()
                && req.id.as_ref().is_none_or(|id| seen.insert(id.clone()))
            {
                scan.requests.push((req.model, req.tokens));
            }
        }
        scan
    }

    fn note_time(&mut self, record: &Record) {
        let Some(ms) = record
            .timestamp
            .as_deref()
            .map(parse_ts)
            .filter(|ms| *ms > 0)
        else {
            return;
        };
        self.first_ms = Some(self.first_ms.map_or(ms, |first| first.min(ms)));
        self.last_ms = Some(self.last_ms.map_or(ms, |last| last.max(ms)));
    }
}

fn is_compaction(record: &Record) -> bool {
    record.kind.as_deref() == Some("system")
        && record.subtype.as_deref() == Some("compact_boundary")
}

#[cfg(test)]
mod tests {
    use super::related;

    #[test]
    fn a_recorded_directory_matches_under_another_spelling() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(sub.as_path()).expect("create sub");
        std::fs::create_dir_all(dir.path().join("other")).expect("create other");
        let spelled = dir.path().join("other").join("..").join("sub");
        assert!(related(&sub, &spelled));
        assert!(!related(&sub, &dir.path().join("other")));
    }
}
