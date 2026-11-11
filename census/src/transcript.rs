// SPDX-License-Identifier: MPL-2.0
//! Reads the records of Claude Code transcripts, the one record model that the usage, session,
//! context and ledger readers share, and lists the transcript files.

use crate::usage::Tokens;
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::fs::Metadata;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Lines of a transcript searched for its recorded working directory.
const CWD_LINES: usize = 50;
/// The model that Claude Code gives the messages it writes for aborts and errors.
const SYNTHETIC: &str = "<synthetic>";

/// The fields of one transcript record that census reads. Other fields are skipped.
#[derive(Deserialize)]
pub(crate) struct Record<'a> {
    #[serde(rename = "type")]
    pub kind: Option<&'a str>,
    pub subtype: Option<&'a str>,
    pub cwd: Option<String>,
    pub timestamp: Option<&'a str>,
    /// Set on the records of a subagent transcript: the role it was spawned as.
    #[serde(rename = "attributionAgent")]
    pub attribution_agent: Option<String>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(rename = "agentId")]
    pub agent_id: Option<String>,
    #[serde(rename = "isMeta")]
    pub is_meta: Option<bool>,
    #[serde(rename = "compactMetadata")]
    pub compact: Option<CompactMeta>,
    #[serde(borrow)]
    pub message: Option<Message<'a>>,
    pub attachment: Option<Attachment>,
}

/// The context before and after a compaction, as Claude Code writes them on a `compact_boundary`.
#[derive(Deserialize)]
pub(crate) struct CompactMeta {
    #[serde(rename = "preTokens")]
    pub pre_tokens: Option<u64>,
    #[serde(rename = "postTokens")]
    pub post_tokens: Option<u64>,
}

/// The fields of an attachment record that the readers use.
#[derive(Deserialize)]
pub(crate) struct Attachment {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(rename = "toolUseID")]
    pub tool_use_id: Option<String>,
    /// Any JSON shape: an attachment must not drop the record it belongs to.
    pub content: Option<Value>,
}

#[derive(Deserialize)]
pub(crate) struct Message<'a> {
    pub id: Option<String>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
    /// The content blocks, left raw: only the tool readers look inside them.
    #[serde(borrow)]
    pub content: Option<&'a RawValue>,
}

#[derive(Deserialize)]
pub(crate) struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

/// One API request of a transcript.
pub(crate) struct Request {
    pub id: Option<String>,
    pub model: String,
    pub tokens: Tokens,
}

impl Usage {
    /// Splits the cache writes by TTL when the record gives the split, as Claude Code does.
    fn tokens(&self) -> Tokens {
        let (cache_write_5m, cache_write_1h) = match &self.cache_creation {
            Some(c) => (c.ephemeral_5m_input_tokens, c.ephemeral_1h_input_tokens),
            None => (self.cache_creation_input_tokens, 0),
        };
        Tokens {
            input: self.input_tokens,
            cache_read: self.cache_read_input_tokens,
            cache_write_5m,
            cache_write_1h,
            output: self.output_tokens,
            requests: 1,
        }
    }
}

impl Record<'_> {
    /// The API request this record carries: an assistant message with usage whose model is not
    /// `<synthetic>`. Other records give `None`.
    pub fn request(&self) -> Option<Request> {
        if self.kind != Some("assistant") {
            return None;
        }
        let message = self.message.as_ref()?;
        let usage = message.usage.as_ref()?;
        let model = message.model.clone().unwrap_or_default();
        if model == SYNTHETIC {
            return None;
        }
        Some(Request {
            id: message.id.clone(),
            model,
            tokens: usage.tokens(),
        })
    }

    /// Returns the tokens before and after this compaction, or `None` unless this is a
    /// `compact_boundary` record that gives both.
    pub fn compaction(&self) -> Option<(u64, u64)> {
        if self.kind != Some("system") || self.subtype != Some("compact_boundary") {
            return None;
        }
        let meta = self.compact.as_ref()?;
        Some((meta.pre_tokens?, meta.post_tokens?))
    }
}

/// The records of a transcript, in file order; lines that are not JSON records are skipped.
pub(crate) fn records(text: &str) -> impl DoubleEndedIterator<Item = Record<'_>> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Record<'_>>(line).ok())
}

/// The working directory the transcript records, found in its first lines.
pub(crate) fn cwd_of(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    BufReader::new(file)
        .lines()
        .take(CWD_LINES)
        .map_while(Result::ok)
        .find_map(|line| serde_json::from_str::<Record<'_>>(&line).ok()?.cwd)
        .filter(|cwd| !cwd.is_empty())
}

/// Lists the `*.jsonl` files below `roots`, subagent transcripts included, sorted by path. Symbolic
/// links are not followed, and directories that cannot be read are skipped.
pub(crate) fn list_files(roots: &[PathBuf]) -> Vec<(PathBuf, Metadata)> {
    let mut files = Vec::new();
    let mut dirs = roots.to_vec();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file()
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".jsonl"))
                && let Ok(meta) = entry.metadata()
            {
                files.push((path, meta));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}
