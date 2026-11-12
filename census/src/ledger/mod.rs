// SPDX-License-Identifier: MPL-2.0
//! Keeps an incremental index of Claude Code transcripts: one row per API request for the recent
//! days, hourly aggregates for the rest.
//!
//! State lives in one directory: `index.json` (read offsets per transcript), `rows.jsonl`,
//! `hourly.jsonl` (first line `{"folded_before":T}`, rows before `T` live only here),
//! `digest.json` (the priced rows and aggregates the limits read, see `digest`), and `lock`.

mod digest;
mod error;
mod fold;
mod model;
mod parse;
mod query;
mod scan;
mod state;
mod update;

pub use digest::Priced;
pub use error::LedgerError;
pub use fold::PruneReport;
pub use model::{Filter, Hourly, Row, Tokens, family_of};

use state::Layout;
use std::cell::OnceCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
    /// When set, the update stops reading at this time, keeps what it read and reports
    /// [`UpdateReport::partial`]. The next update continues from there.
    pub deadline: Option<Instant>,
    /// The transcript of the running session. While the last complete sweep is less than a minute
    /// old, an update reads only this file; without it every update sweeps all transcripts.
    pub current: Option<PathBuf>,
}

impl Opts {
    pub fn new(roots: Vec<PathBuf>, state_dir: PathBuf, now: u64) -> Self {
        Opts {
            roots,
            state_dir,
            now,
            retention_s: RETENTION_DAYS * 86_400,
            deadline: None,
            current: None,
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
    /// True when the deadline stopped the update before all transcript bytes were read.
    pub partial: bool,
}

/// Gives read access to the ledger state after an update.
pub struct Ledger {
    layout: Layout,
    report: UpdateReport,
    /// The digest the update just wrote, so the first `priced` call does not read it back.
    digest: Option<digest::Digest>,
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
        let (report, digest) = match state::try_lock(&layout.lock, LOCK_WAIT)? {
            Some(_lock) => update::run(&layout, opts)?,
            None => (UpdateReport::default(), None),
        };
        Ok(Ledger {
            layout,
            report,
            digest,
            rows: OnceCell::new(),
            hourly: OnceCell::new(),
        })
    }

    /// Returns the ledger as it is in `state_dir`, without reading any transcript. The writer lock
    /// is taken only to save a missing or stale digest.
    pub fn open(state_dir: &std::path::Path) -> Ledger {
        Ledger {
            layout: Layout::new(state_dir),
            report: UpdateReport::default(),
            digest: None,
            rows: OnceCell::new(),
            hourly: OnceCell::new(),
        }
    }

    pub fn report(&self) -> UpdateReport {
        self.report
    }

    /// Returns the raw rows not yet folded into hourly aggregates, each message id once.
    ///
    /// # Errors
    /// Fails when `rows.jsonl` cannot be read.
    pub fn rows(&self) -> Result<&[Row], LedgerError> {
        self.load()?;
        Ok(self.rows.get().map_or(&[], Vec::as_slice))
    }

    /// Reads the hourly aggregates and the raw rows together under the shared lock, so a fold
    /// running in between cannot make a row count twice or not at all. Without the lock (a long
    /// writer), the hourly file is read first and rows it already holds are dropped.
    fn load(&self) -> Result<(), LedgerError> {
        if self.rows.get().is_some() && self.hourly.get().is_some() {
            return Ok(());
        }
        let _guard = state::lock_shared(&self.layout.lock, LOCK_WAIT);
        let (folded, hourly) = state::load_hourly(&self.layout.hourly)?;
        let rows = state::load_rows(&self.layout.rows, folded)?;
        let _ = self.hourly.set(hourly);
        let _ = self.rows.set(rows);
        Ok(())
    }

    /// Returns the hourly aggregates, ordered by hour.
    ///
    /// # Errors
    /// Fails when `hourly.jsonl` cannot be read.
    pub fn hourly(&self) -> Result<&[Hourly], LedgerError> {
        self.load()?;
        Ok(self.hourly.get().map_or(&[], Vec::as_slice))
    }

    /// Returns the list-price USD of the rows and aggregates. The digest is used when it is
    /// current. Otherwise it is built from the ledger files, and saved when the writer lock is free.
    ///
    /// # Errors
    /// Fails when a ledger file cannot be read.
    pub fn priced(&self) -> Result<Priced, LedgerError> {
        if let Some(digest) = &self.digest {
            return Ok(digest.to_priced());
        }
        if let Some(digest) = digest::read(&self.layout) {
            return Ok(digest.to_priced());
        }
        if let Some(_lock) = state::try_lock(&self.layout.lock, LOCK_WAIT)? {
            let digest = digest::build(&self.layout, digest::unix_now())?;
            // A failed save leaves the digest stale, and the next read builds it again.
            let _ = digest::write(&self.layout, &digest);
            return Ok(digest.to_priced());
        }
        let _guard = state::lock_shared(&self.layout.lock, LOCK_WAIT);
        Ok(digest::build(&self.layout, digest::unix_now())?.to_priced())
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
        if !dry_run {
            // The digest holds the hourly aggregates too. The next read builds it again.
            let _ = std::fs::remove_file(&self.layout.digest);
        }
        self.hourly.take();
        self.digest = None;
        Ok(report)
    }
}
