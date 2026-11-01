// SPDX-License-Identifier: MPL-2.0
//! Reads the records of Claude Code transcripts that the session and context views need.

use crate::usage::Tokens;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Lines of a transcript searched for its recorded working directory.
const CWD_LINES: usize = 50;

/// The fields of one transcript record that census reads. Other fields are skipped.
#[derive(Deserialize)]
pub(crate) struct Record {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub subtype: Option<String>,
    pub cwd: Option<String>,
    pub timestamp: Option<String>,
    /// Set on the records of a subagent transcript: the role it was spawned as.
    #[serde(rename = "attributionAgent")]
    pub attribution_agent: Option<String>,
    pub message: Option<Message>,
}

#[derive(Deserialize)]
pub(crate) struct Message {
    pub id: Option<String>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
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

impl Record {
    /// The API request this record carries, or `None` for other records and synthetic messages.
    pub fn request(&self) -> Option<Request> {
        if self.kind.as_deref() != Some("assistant") {
            return None;
        }
        let message = self.message.as_ref()?;
        let usage = message.usage.as_ref()?;
        let model = message.model.clone().unwrap_or_default();
        if model == "<synthetic>" {
            return None;
        }
        Some(Request {
            id: message.id.clone(),
            model,
            tokens: usage.tokens(),
        })
    }
}

/// The records of a transcript, in file order; lines that are not JSON records are skipped.
pub(crate) fn records(text: &str) -> impl DoubleEndedIterator<Item = Record> + '_ {
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
}

/// The working directory the transcript records, found in its first lines.
pub(crate) fn cwd_of(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    BufReader::new(file)
        .lines()
        .take(CWD_LINES)
        .map_while(Result::ok)
        .find_map(|line| serde_json::from_str::<Record>(&line).ok()?.cwd)
        .filter(|cwd| !cwd.is_empty())
}
