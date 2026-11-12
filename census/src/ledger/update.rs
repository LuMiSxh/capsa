// SPDX-License-Identifier: MPL-2.0
//! Brings the ledger up to date with the transcripts. The caller holds the writer lock.

use super::digest::{self, Digest};
use super::error::LedgerError;
use super::model::{Row, floor_hour};
use super::scan;
use super::state::{self, Index, Layout};
use super::{Opts, UpdateReport};
use crate::transcript;
use std::collections::HashSet;
use std::fs::Metadata;
use std::path::PathBuf;
use std::time::Instant;

type Result<T> = std::result::Result<T, LedgerError>;

/// Raw rows may outlive the retention by this much before a fold is worth rewriting the file.
const FOLD_SLACK_S: u64 = 6 * 3600;

/// Seconds after a complete sweep during which an update reads only the current session's
/// transcript. A transcript of another session that changes in that window is read by the next
/// full sweep, so its rows lag by at most this long. Nothing is lost or counted twice, because each
/// file resumes from the offset its index entry records. A sweep costs time in the number of files,
/// so the bound keeps it to about one a minute.
const SWEEP_FRESH_S: u64 = 60;

/// Returns the report and, when the update wrote or checked it, the digest as it now stands.
pub(super) fn run(layout: &Layout, opts: &Opts) -> Result<(UpdateReport, Option<Digest>)> {
    let mut report = UpdateReport::default();
    let mut index = state::load_index(&layout.index);
    let folded_before = state::folded_before(&layout.hourly);
    let target = floor_hour(opts.now.saturating_sub(opts.retention_s));
    let current = current_only(opts, &index);
    let full = current.is_none();
    let (todo, mut dirty) = match current {
        Some(file) => scan::stale(vec![file], &mut index),
        None => scan::pending(transcript::list_files(&opts.roots), &mut index),
    };
    let mut fresh = Vec::new();
    let mut before = None;
    if !todo.is_empty() {
        // The digest holds the ids of the raw rows, so the rows file is not read for them.
        before = digest::read(layout);
        let mut seen: HashSet<String> = match &before {
            Some(digest) => digest.ids(),
            None => state::load_ids(&layout.rows, folded_before)?,
        };
        for (i, (path, meta)) in todo.iter().enumerate() {
            if i > 0 && opts.deadline.is_some_and(|d| Instant::now() >= d) {
                report.partial = true;
                break;
            }
            let Some(st) = index.files.get_mut(path.to_string_lossy().as_ref()) else {
                continue;
            };
            let scanned = scan::scan_file(
                path,
                meta,
                st,
                folded_before,
                &mut seen,
                &mut fresh,
                opts.deadline,
            );
            if let Ok(s) = scanned {
                report.bytes_read += s.bytes;
                report.files_scanned += 1;
                if s.partial {
                    report.partial = true;
                    break;
                }
            }
        }
        dirty = true;
    }
    if full && !report.partial && !is_fresh(opts, &index) {
        // Written at most once per window: an older stamp only ends the window sooner, and a sweep
        // on every call would rewrite the index on every call.
        index.swept_at = Some(opts.now);
        dirty = true;
    }
    let (old, keep): (Vec<_>, Vec<_>) = fresh.into_iter().partition(|r| r.t < target);
    state::append_rows(&layout.rows, &keep)?;
    report.rows_added = keep.len() + old.len();
    let oldest = keep.iter().map(|r| r.t).chain(index.oldest_t).min();
    index.oldest_t = oldest;
    let folded = !old.is_empty() || oldest.is_some_and(|t| t + FOLD_SLACK_S < target);
    if folded {
        report.folded = super::fold::fold(layout, target, &old, &mut index)?;
        dirty = true;
    }
    let digest = if folded || !todo.is_empty() {
        // The digest is a cache: when it cannot be written, the next read builds it again.
        refresh_digest(layout, before, &keep, folded, opts.now).ok()
    } else {
        None
    };
    if dirty {
        state::save_index(&layout.index, &index)?;
    }
    report.updated = true;
    Ok((report, digest))
}

/// Returns the current session's transcript when the last complete sweep is fresh. Without a
/// current transcript, or with one outside the roots, the update sweeps every transcript.
fn current_only(opts: &Opts, index: &Index) -> Option<(PathBuf, Metadata)> {
    if !is_fresh(opts, index) {
        return None;
    }
    transcript::one_file(opts.current.as_deref()?, &opts.roots)
}

/// Returns whether a complete sweep began less than [`SWEEP_FRESH_S`] before `now`. A missing sweep
/// time (a new or torn index) and a sweep time after `now` (the clock went back) are not fresh.
fn is_fresh(opts: &Opts, index: &Index) -> bool {
    index
        .swept_at
        .is_some_and(|s| s <= opts.now && opts.now - s < SWEEP_FRESH_S)
}

/// Brings the digest up to date after an update and returns it. Rows appended by the update join
/// the digest read before it. After a fold, or when the digest was missing, stale or did not
/// match the rows file, it is built from the files.
///
/// The caller holds the exclusive writer lock from before the digest was read until after it is
/// written, and every writer of `rows.jsonl` and `digest.json` takes that lock. So the rows the
/// digest read before the update covers are exactly the rows in the file, `keep` is exactly what
/// was appended since, and a second process can only start folding after this digest is in
/// place, where it reads these rows as already counted. A reader without the lock sees a digest
/// whose key does not match a file that is being appended and builds its own.
fn refresh_digest(
    layout: &Layout,
    before: Option<Digest>,
    keep: &[Row],
    folded: bool,
    now: u64,
) -> Result<Digest> {
    match before {
        Some(mut digest) if !folded => {
            if !keep.is_empty() {
                digest.append(keep, digest::RowsKey::of(&layout.rows), now);
                digest::write(layout, &digest)?;
            }
            Ok(digest)
        }
        _ => {
            let digest = digest::build(layout, now)?;
            digest::write(layout, &digest)?;
            Ok(digest)
        }
    }
}
