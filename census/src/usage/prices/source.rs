// SPDX-License-Identifier: MPL-2.0
//! Keeps the price cache in the census state directory and fetches LiteLLM's price list into it.
//! The snapshot built into the binary serves a first run that has no cache.

use super::table::{self, Table};
use capsa_core::fs::write_atomic;
use capsa_core::log::{self, Level};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CACHE: &str = "prices.json";
/// Refresh warnings go here, not to `hook.jsonl`, which `census self doctor` reads as hook errors.
pub const LOG: &str = "prices.jsonl";
pub const LOG_LIMIT: u64 = 256 * 1024;
const URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const DAY_S: u64 = 86_400;
/// How long a failed fetch holds the next one, in seconds.
const RETRY_S: u64 = 3_600;
/// Unix seconds of the last failed fetch, beside the cache, so the hold survives a restart.
const ATTEMPT: &str = "prices.attempt";
/// The table of the build. `census/AGENTS.md` names the command that regenerates it.
const SNAPSHOT: &str = include_str!("../../../data/prices.snapshot.json");

/// Why a refresh did not write the cache. The cache is kept, and the reason is logged as a warning.
#[derive(Debug, thiserror::Error)]
pub(super) enum PriceError {
    #[error("cannot fetch the price list: {0}")]
    Fetch(String),
    #[error("the price list is not a JSON object")]
    Parse(#[source] serde_json::Error),
    #[error("the price list names no model census prices")]
    Empty,
    #[error("cannot write {}", .0.display())]
    Write(PathBuf, #[source] io::Error),
}

/// Returns the cached table when it reads and has models, else the snapshot.
pub(super) fn load(dir: &Path) -> Table {
    read_cache(dir)
        .filter(|t| !t.models.is_empty())
        .unwrap_or_else(snapshot)
}

/// Returns the table built into the binary.
pub(super) fn snapshot() -> Table {
    serde_json::from_str(SNAPSHOT).unwrap_or_default()
}

/// Fetches the price list into `dir` when the cache is stale, and returns whether it was written.
pub(super) fn refresh(dir: &Path, now: u64, days: u64) -> bool {
    refresh_with(dir, now, days, fetch)
}

/// Fetches with `fetch` when the cache is stale and no recent failure holds the fetch. A failed
/// fetch, parse or write keeps the cache and starts the hold; a success clears it.
pub(super) fn refresh_with(
    dir: &Path,
    now: u64,
    days: u64,
    fetch: impl FnOnce() -> Result<String, PriceError>,
) -> bool {
    if !is_stale(read_cache(dir).as_ref(), now, days) || backing_off(dir, now) {
        return false;
    }
    let result = fetch()
        .and_then(|text| table::trim(&text, now).map_err(PriceError::Parse))
        .and_then(non_empty)
        .and_then(|t| write_cache(dir, &t));
    match result {
        Ok(()) => {
            let _ = std::fs::remove_file(dir.join(ATTEMPT));
            true
        }
        Err(e) => {
            warn(dir, &e);
            let _ = write_atomic(&dir.join(ATTEMPT), now.to_string().as_bytes());
            false
        }
    }
}

/// Whether a failed fetch less than `RETRY_S` before `now` holds the next one. A record later than
/// `now` is ignored, so a clock that went back does not hold the fetch.
fn backing_off(dir: &Path, now: u64) -> bool {
    std::fs::read_to_string(dir.join(ATTEMPT))
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .is_some_and(|at| at <= now && now - at < RETRY_S)
}

/// Whether the cache is missing, unreadable or at least `days` old at `now`. A `days` below 1 counts
/// as 1.
pub(super) fn is_stale(cache: Option<&Table>, now: u64, days: u64) -> bool {
    cache.is_none_or(|t| now.saturating_sub(t.fetched_at) >= days.max(1).saturating_mul(DAY_S))
}

fn non_empty(t: Table) -> Result<Table, PriceError> {
    if t.models.is_empty() {
        Err(PriceError::Empty)
    } else {
        Ok(t)
    }
}

fn read_cache(dir: &Path) -> Option<Table> {
    serde_json::from_slice(&std::fs::read(dir.join(CACHE)).ok()?).ok()
}

/// Replaces the cache with `t`.
fn write_cache(dir: &Path, t: &Table) -> Result<(), PriceError> {
    let path = dir.join(CACHE);
    let write = |e| PriceError::Write(path.clone(), e);
    let bytes = serde_json::to_vec(t).map_err(|e| write(io::Error::other(e)))?;
    std::fs::create_dir_all(dir).map_err(write)?;
    write_atomic(&path, &bytes).map_err(write)
}

/// Appends why a refresh failed to `prices.jsonl`.
fn warn(dir: &Path, e: &PriceError) {
    let _ = std::fs::create_dir_all(dir);
    let fields = serde_json::json!({ "message": e.to_string() });
    let _ = log::append(
        &dir.join(LOG),
        LOG_LIMIT,
        Level::Warn,
        "prices.refresh",
        &fields,
    );
}

/// Downloads LiteLLM's price file. The timeout bounds the whole request.
fn fetch() -> Result<String, PriceError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(URL)
        .call()
        .map_err(|e| PriceError::Fetch(e.to_string()))?;
    response
        .body_mut()
        .read_to_string()
        .map_err(|e| PriceError::Fetch(e.to_string()))
}
