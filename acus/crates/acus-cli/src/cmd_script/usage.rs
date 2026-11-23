// SPDX-License-Identifier: MPL-2.0
//! Owns `script.jsonl`, the log of script runs, saved and inline, beside `guard.jsonl`.

use super::store::Scope;
use crate::config::Config;
use capsa_core::log::{self, Level};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const NAME: &str = "script.jsonl";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const EVENT: &str = "run";

/// One run of a saved script.
#[derive(Serialize, Deserialize)]
pub(super) struct Run {
    pub(super) name: String,
    pub(super) version: u32,
    pub(super) scope: Scope,
    pub(super) cwd: String,
    pub(super) ms: u64,
    pub(super) ok: bool,
}

/// One run of an inline program from `-e` or stdin. It has no saved name, so it does not parse as
/// a `Run`, and the census reads it apart by `inline`.
#[derive(Serialize)]
pub(super) struct Inline {
    pub(super) inline: bool,
    pub(super) cwd: String,
    pub(super) ms: u64,
    pub(super) ok: bool,
    pub(super) calls: u64,
}

pub(super) fn path(cfg: &Config) -> PathBuf {
    capsa_core::paths::log_file("acus", NAME).unwrap_or_else(|| cfg.path.with_file_name(NAME))
}

/// Appends one run, saved or inline; a log that cannot be written never fails the script.
pub(super) fn record<T: Serialize>(log: &Path, run: &T) {
    let _ = log::append(log, MAX_BYTES, Level::Info, EVENT, run);
}

/// A run with the time it started, in unix seconds.
pub(super) struct Used {
    pub(super) ts: u64,
    pub(super) run: Run,
}

/// Returns every logged run, oldest first.
pub(super) fn runs(cfg: &Config) -> Vec<Used> {
    log::read::<Run>(&path(cfg))
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.event.as_deref() == Some(EVENT))
        .map(|e| Used {
            ts: e.ts,
            run: e.fields,
        })
        .collect()
}

/// Reports whether the run happened in `dir` or below it.
pub(super) fn under(run: &Run, dir: &Path) -> bool {
    Path::new(&run.cwd).starts_with(dir)
}
