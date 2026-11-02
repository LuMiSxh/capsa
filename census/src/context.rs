// SPDX-License-Identifier: MPL-2.0
//! Owns the context size of a session against the point where Claude Code compacts it.

use crate::transcript;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Context window of models that do not have the large one.
const DEFAULT_WINDOW: u64 = 200_000;
/// Context window of the 5-series, Fable, and Opus or Sonnet 4.6 and later.
const LARGE_WINDOW: u64 = 1_000_000;

/// The size at which Claude Code compacts, and where its value came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Threshold {
    pub tokens: u64,
    /// What set the value, such as `autoCompactWindow in settings.json`.
    pub source: String,
}

/// The context of a session: tokens in use against the threshold.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContextUse {
    pub tokens: u64,
    pub model: Option<String>,
    pub threshold: Threshold,
    /// Share of the threshold in use, in percent.
    pub percent: f64,
    /// Tokens between the current context and the threshold, 0 once it is passed.
    pub left: u64,
}

/// Returns the context of `tokens` for `model` against the threshold of this process.
pub fn measure(tokens: u64, model: Option<&str>) -> ContextUse {
    measure_with(tokens, model, settings_window(&claude_config_dir()))
}

/// Like [`measure`], with the `autoCompactWindow` of the settings already read by the caller.
pub fn measure_with(tokens: u64, model: Option<&str>, settings: Option<u64>) -> ContextUse {
    let threshold = threshold(model, |k| std::env::var(k).ok(), settings);
    at(tokens, model, threshold)
}

/// Returns the context of `tokens` for `model` against `threshold`.
pub fn at(tokens: u64, model: Option<&str>, threshold: Threshold) -> ContextUse {
    ContextUse {
        tokens,
        model: model.map(str::to_owned),
        percent: 100.0 * tokens as f64 / threshold.tokens as f64,
        left: threshold.tokens.saturating_sub(tokens),
        threshold,
    }
}

/// Resolves the compaction threshold: `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, else the
/// `autoCompactWindow` setting, else the model's window; capped at the model's window, then
/// lowered by `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`.
pub fn threshold(
    model: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
    settings: Option<u64>,
) -> Threshold {
    let window = model_window(model);
    let positive = |s: String| s.trim().parse::<u64>().ok().filter(|n| *n > 0);
    let (base, source) = match (
        env("CLAUDE_CODE_AUTO_COMPACT_WINDOW").and_then(positive),
        settings.filter(|n| *n > 0),
    ) {
        (Some(n), _) => (n, "CLAUDE_CODE_AUTO_COMPACT_WINDOW".to_owned()),
        (None, Some(n)) => (n, "autoCompactWindow in settings.json".to_owned()),
        (None, None) => (window, "default (approximate)".to_owned()),
    };
    let mut tokens = base.min(window);
    let pct = env("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|p| (1..=100).contains(p));
    let Some(pct) = pct else {
        return Threshold { tokens, source };
    };
    tokens = (tokens * pct / 100).max(1);
    Threshold {
        tokens,
        source: format!("{source}, CLAUDE_AUTOCOMPACT_PCT_OVERRIDE {pct}%"),
    }
}

/// Returns the context window of a model id: 1,000,000 for the 5-series, Fable, and Opus or
/// Sonnet 4.6 and later; 200,000 for everything else and for an unknown model.
pub fn model_window(model: Option<&str>) -> u64 {
    let Some(model) = model else {
        return DEFAULT_WINDOW;
    };
    let model = model.to_ascii_lowercase();
    let fifth_series = model.strip_prefix("claude-").and_then(|rest| {
        rest.split('-')
            .find(|p| p.chars().all(|c| c.is_ascii_digit()))
    }) == Some("5");
    let newer_family = ["opus", "sonnet"]
        .iter()
        .find_map(|family| family_version(&model, family))
        .is_some_and(|version| version >= (4, 6));
    if fifth_series || newer_family || model.contains("fable") {
        LARGE_WINDOW
    } else {
        DEFAULT_WINDOW
    }
}

/// The `(major, minor)` version written after a family name, as in `claude-opus-4-6`.
fn family_version(model: &str, family: &str) -> Option<(u32, u32)> {
    let rest = model.split_once(family)?.1.trim_start_matches('-');
    let mut parts = rest.split('-');
    // Dates such as `20241022` are longer than a version part and are not versions.
    let part = |p: Option<&str>| p.filter(|p| p.len() <= 2)?.parse::<u32>().ok();
    Some((part(parts.next())?, part(parts.next())?))
}

/// The configuration directory of Claude Code: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".claude")))
        .unwrap_or_default()
}

/// Returns the `autoCompactWindow` set in `dir/settings.json`, if any.
pub fn settings_window(dir: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(dir.join("settings.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("autoCompactWindow")?
        .as_u64()
}

/// Bytes read from the end of a transcript to find its last API request and its last compaction.
const TAIL_BYTES: u64 = 512 * 1024;
/// Larger tails, read in turn only while the shorter ones hold no API request. Past the last one
/// the context of the session is unknown.
const FALLBACK_BYTES: [u64; 2] = [4 * 1024 * 1024, 16 * 1024 * 1024];

/// What the end of a transcript says about its session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ends {
    /// Model and context tokens of the last API request.
    pub context: Option<(String, u64)>,
    /// Tokens before and after the last compaction, when the end of the transcript records one.
    pub compacted: Option<(u64, u64)>,
}

/// Reads the end of a transcript once and returns its last API request and its last compaction.
/// The compaction is looked for in the first tail only.
pub fn transcript_ends(path: &Path) -> Ends {
    let Some(mut tail) = read_tail(path, TAIL_BYTES) else {
        return Ends::default();
    };
    let mut ends = scan_ends(&String::from_utf8_lossy(&tail.bytes));
    for limit in FALLBACK_BYTES {
        if ends.context.is_some() || tail.whole {
            break;
        }
        let Some(next) = read_tail(path, limit) else {
            break;
        };
        ends.context = find_last(&String::from_utf8_lossy(&next.bytes));
        tail = next;
    }
    ends
}

/// Returns the model and context tokens of the last API request in a transcript: the input,
/// cache reads and cache writes of that request.
pub fn last_context(transcript: &Path) -> Option<(String, u64)> {
    transcript_ends(transcript).context
}

/// Returns the last API request and the last compaction of `text` in one pass from its end.
fn scan_ends(text: &str) -> Ends {
    let mut ends = Ends::default();
    for record in transcript::records(text).rev() {
        if ends.context.is_none() {
            ends.context = record
                .request()
                .map(|req| (req.model, req.tokens.context()));
        }
        if ends.compacted.is_none() {
            ends.compacted = record.compaction();
        }
        if ends.context.is_some() && ends.compacted.is_some() {
            break;
        }
    }
    ends
}

fn find_last(text: &str) -> Option<(String, u64)> {
    transcript::records(text)
        .rev()
        .find_map(|record| record.request())
        .map(|req| (req.model, req.tokens.context()))
}

/// The last bytes of a transcript, and whether they are the whole file.
struct Tail {
    bytes: Vec<u8>,
    whole: bool,
}

/// Reads the last `limit` bytes of a file, or the whole file when it is shorter. The cut may split
/// a record or a character; the broken first line fails to parse and is skipped.
fn read_tail(path: &Path, limit: u64) -> Option<Tail> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let whole = len <= limit;
    if !whole {
        file.seek(SeekFrom::Start(len - limit)).ok()?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(Tail { bytes, whole })
}
