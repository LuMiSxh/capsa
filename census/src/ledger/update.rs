// SPDX-License-Identifier: MPL-2.0
//! Brings the ledger up to date with the transcripts. The caller holds the writer lock.

use super::digest::{self, Digest};
use super::error::LedgerError;
use super::model::{Row, floor_hour};
use super::scan;
use super::state::{self, Layout};
use super::{Opts, UpdateReport};
use crate::transcript;
use std::collections::HashSet;
use std::time::Instant;

type Result<T> = std::result::Result<T, LedgerError>;

/// Raw rows may outlive the retention by this much before a fold is worth rewriting the file.
const FOLD_SLACK_S: u64 = 6 * 3600;

pub(super) fn run(layout: &Layout, opts: &Opts) -> Result<UpdateReport> {
    let mut report = UpdateReport::default();
    let mut index = state::load_index(&layout.index);
    let folded_before = state::folded_before(&layout.hourly);
    let target = floor_hour(opts.now.saturating_sub(opts.retention_s));
    let (todo, mut dirty) = scan::pending(transcript::list_files(&opts.roots), &mut index);
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
    if folded || !todo.is_empty() {
        // The digest is a cache: when it cannot be written, the next read builds it again.
        let _ = refresh_digest(layout, before, &keep, folded, opts.now);
    }
    if dirty {
        state::save_index(&layout.index, &index)?;
    }
    report.updated = true;
    Ok(report)
}

/// Brings the digest up to date after an update. Rows appended by the update join the digest
/// read before it. After a fold, or when the digest was missing or stale, it is built from the files.
fn refresh_digest(
    layout: &Layout,
    before: Option<Digest>,
    keep: &[Row],
    folded: bool,
    now: u64,
) -> Result<()> {
    match before {
        Some(mut digest) if !folded => {
            if !keep.is_empty() {
                digest.append(keep, digest::file_len(&layout.rows), now);
                digest::write(layout, &digest)?;
            }
            Ok(())
        }
        _ => digest::write(layout, &digest::build(layout, now)?),
    }
}
