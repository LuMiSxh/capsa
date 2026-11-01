// SPDX-License-Identifier: MPL-2.0
//! Keeps an incremental index of Claude Code transcripts: one row per API request for the recent
//! days, hourly aggregates for the rest.
//!
//! State lives in one directory: `index.json` (read offsets per transcript), `rows.jsonl`,
//! `hourly.jsonl` (first line `{"folded_before":T}`, rows before `T` live only here), and `lock`.

mod error;
mod fold;
mod model;
mod parse;
mod query;
mod scan;
mod state;
mod update;

pub use error::LedgerError;
pub use fold::PruneReport;
pub use model::{Filter, Hourly, Row, Tokens, family_of};

use state::Layout;
use std::cell::OnceCell;
use std::path::PathBuf;
use std::time::Duration;

/// Days of raw rows kept; the weekly limit window needs seven.
pub const RETENTION_DAYS: u64 = 8;

const LOCK_WAIT: Duration = Duration::from_millis(20);

/// Configures one ledger update.
#[derive(Clone, Debug)]
pub struct Opts {
    /// Directories searched recursively for `*.jsonl` transcripts.
    pub roots: Vec<PathBuf>,
    /// Directory of the ledger state files.
    pub state_dir: PathBuf,
    /// Current unix time in seconds.
    pub now: u64,
    /// Seconds raw rows are kept before they fold into hourly aggregates.
    pub retention_s: u64,
}

impl Opts {
    pub fn new(roots: Vec<PathBuf>, state_dir: PathBuf, now: u64) -> Self {
        Opts {
            roots,
            state_dir,
            now,
            retention_s: RETENTION_DAYS * 86_400,
        }
    }

    /// Uses the default Claude Code projects directory and the census state directory.
    pub fn defaults(now: u64) -> Self {
        Opts::new(default_roots(), capsa_core::paths::state_dir("census"), now)
    }
}

/// Returns `$CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`.
pub fn default_roots() -> Vec<PathBuf> {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".claude")));
    base.map(|b| b.join("projects")).into_iter().collect()
}

/// Describes what one update did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateReport {
    /// False when another process held the lock and the ledger was read as it is.
    pub updated: bool,
    pub files_scanned: usize,
    pub bytes_read: u64,
    pub rows_added: usize,
    /// Rows moved into hourly aggregates.
    pub folded: usize,
}

/// Gives read access to the ledger state after an update.
pub struct Ledger {
    layout: Layout,
    report: UpdateReport,
    rows: OnceCell<Vec<Row>>,
    hourly: OnceCell<Vec<Hourly>>,
}

impl Ledger {
    /// Updates the ledger from the transcripts, then returns it. When another process holds the
    /// writer lock for more than about 20 ms, the current state is returned without an update.
    ///
    /// # Errors
    /// Fails when the state directory or a state file cannot be written.
    pub fn open_and_update(opts: &Opts) -> Result<Ledger, LedgerError> {
        let layout = Layout::new(&opts.state_dir);
        std::fs::create_dir_all(&layout.dir).map_err(state::io_at(&layout.dir))?;
        let report = match state::try_lock(&layout.lock, LOCK_WAIT)? {
            Some(_lock) => update::run(&layout, opts)?,
            None => UpdateReport::default(),
        };
        Ok(Ledger {
            layout,
            report,
            rows: OnceCell::new(),
            hourly: OnceCell::new(),
        })
    }

    pub fn report(&self) -> UpdateReport {
        self.report
    }

    /// Returns the raw rows not yet folded into hourly aggregates, each message id once.
    ///
    /// # Errors
    /// Fails when `rows.jsonl` cannot be read.
    pub fn rows(&self) -> Result<&[Row], LedgerError> {
        if self.rows.get().is_none() {
            let folded = state::folded_before(&self.layout.hourly);
            let _ = self.rows.set(state::load_rows(&self.layout.rows, folded)?);
        }
        Ok(self.rows.get().map_or(&[], Vec::as_slice))
    }

    /// Returns the hourly aggregates, ordered by hour.
    ///
    /// # Errors
    /// Fails when `hourly.jsonl` cannot be read.
    pub fn hourly(&self) -> Result<&[Hourly], LedgerError> {
        if self.hourly.get().is_none() {
            let (_, hourly) = state::load_hourly(&self.layout.hourly)?;
            let _ = self.hourly.set(hourly);
        }
        Ok(self.hourly.get().map_or(&[], Vec::as_slice))
    }

    /// Counts, and unless `dry_run` removes, hourly aggregates and snapshots older than `before`.
    /// Raw rows are never pruned.
    ///
    /// # Errors
    /// Fails with [`LedgerError::Busy`] when another process holds the lock, or when a file
    /// cannot be rewritten.
    pub fn prune(&mut self, before: u64, dry_run: bool) -> Result<PruneReport, LedgerError> {
        let Some(_lock) = state::try_lock(&self.layout.lock, LOCK_WAIT)? else {
            return Err(LedgerError::Busy);
        };
        let report = fold::prune(&self.layout, before, dry_run)?;
        self.hourly.take();
        Ok(report)
    }
}
