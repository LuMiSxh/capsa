// SPDX-License-Identifier: MPL-2.0
//! Owns the `statusLine` input of Claude Code: the limit snapshots it carries, and the status line
//! census renders from it.

use crate::context;
use capsa_core::census_snapshot::{self, Snapshot, SnapshotError, Source, Window};
use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Bytes read from the end of the snapshot file to find the last snapshot of each window.
const TAIL_BYTES: u64 = 8192;

/// The fields of the statusLine JSON that census reads.
#[derive(Deserialize)]
pub struct Input {
    /// A string in older Claude Code versions, an object with `id` in newer ones.
    pub model: Option<Value>,
    pub context_window: Option<ContextWindow>,
    pub rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
pub struct ContextWindow {
    pub total_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
pub struct RateLimits {
    pub five_hour: Option<Limit>,
    pub seven_day: Option<Limit>,
}

#[derive(Deserialize)]
pub struct Limit {
    pub used_percentage: Option<f64>,
    pub resets_at: Option<u64>,
}

impl Input {
    /// The model id, when the input names one.
    pub fn model_id(&self) -> Option<&str> {
        let model = self.model.as_ref()?;
        model.as_str().or_else(|| model.get("id")?.as_str())
    }
}

/// Parses the statusLine JSON, or returns `None` when the text is not one.
pub fn parse(text: &str) -> Option<Input> {
    serde_json::from_str(text).ok()
}

/// Returns one snapshot per limit window the input carries, stamped `now`. A window without both
/// its percentage and its reset time is skipped.
pub fn snapshots(input: &Input, now: u64) -> Vec<Snapshot> {
    let Some(limits) = &input.rate_limits else {
        return Vec::new();
    };
    [
        (Window::FiveHour, &limits.five_hour),
        (Window::SevenDay, &limits.seven_day),
    ]
    .into_iter()
    .filter_map(|(window, limit)| {
        let limit = limit.as_ref()?;
        Some(Snapshot {
            t: now,
            source: Source::Statusline,
            window,
            used_percentage: limit.used_percentage?,
            resets_at: limit.resets_at?,
        })
    })
    .collect()
}

/// Appends each snapshot that differs from the last one of its window in `file`, and returns how
/// many were written.
///
/// # Errors
/// Fails when the file cannot be written.
pub fn record(file: &Path, snapshots: &[Snapshot]) -> Result<usize, SnapshotError> {
    let last = last_per_window(file);
    let mut written = 0;
    for snapshot in snapshots {
        let unchanged = last
            .iter()
            .find(|l| l.window == snapshot.window)
            .is_some_and(|l| same_reading(l, snapshot));
        if !unchanged {
            census_snapshot::append_to(file, snapshot)?;
            written += 1;
        }
    }
    Ok(written)
}

/// Renders the status line, such as `ctx 44k/400k · 5h 6% · wk 1%`: the context against the
/// compaction threshold, then each limit window of `snapshots`. Empty when there is nothing
/// to show.
pub fn line(input: &Input, snapshots: &[Snapshot]) -> String {
    let mut parts = Vec::new();
    if let Some(tokens) = input
        .context_window
        .as_ref()
        .and_then(|c| c.total_input_tokens)
    {
        let context = context::measure(tokens, input.model_id());
        parts.push(format!(
            "ctx {}k/{}k",
            context.tokens / 1000,
            context.threshold.tokens / 1000
        ));
    }
    for snapshot in snapshots {
        let label = match snapshot.window {
            Window::FiveHour => "5h",
            Window::SevenDay => "wk",
        };
        parts.push(format!("{label} {:.0}%", snapshot.used_percentage));
    }
    parts.join(" · ")
}

fn same_reading(a: &Snapshot, b: &Snapshot) -> bool {
    (a.used_percentage - b.used_percentage).abs() < f64::EPSILON && a.resets_at == b.resets_at
}

/// The last snapshot of each window within the tail of `file`. A missing or unreadable file gives
/// none, so the caller writes its reading.
fn last_per_window(file: &Path) -> Vec<Snapshot> {
    let Ok(text) = read_tail(file) else {
        return Vec::new();
    };
    let mut found: Vec<Snapshot> = Vec::new();
    for snapshot in text
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Snapshot>(line).ok())
    {
        if !found.iter().any(|f| f.window == snapshot.window) {
            found.push(snapshot);
        }
    }
    found
}

/// Reads the last `TAIL_BYTES` of `file` as whole lines.
fn read_tail(file: &Path) -> std::io::Result<String> {
    let mut f = File::open(file)?;
    let start = f.metadata()?.len().saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    // A read that starts mid-file begins with a fragment of a line, which is dropped.
    Ok(if start > 0 {
        text.split_once('\n')
            .map_or_else(String::new, |(_, rest)| rest.to_owned())
    } else {
        text.into_owned()
    })
}
