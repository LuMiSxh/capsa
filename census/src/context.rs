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
    let settings = settings_window(&claude_config_dir());
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

/// Returns the model and context tokens of the last API request in a transcript: the input,
/// cache reads and cache writes of that request.
pub fn last_context(transcript: &Path) -> Option<(String, u64)> {
    let text = std::fs::read_to_string(transcript).ok()?;
    transcript::records(&text)
        .rev()
        .find_map(|record| record.request())
        .map(|req| (req.model, req.tokens.context()))
}
