// SPDX-License-Identifier: MPL-2.0
//! Brings the ledger up to date with the transcripts. The caller holds the writer lock.

use super::error::LedgerError;
use super::model::floor_hour;
use super::scan;
use super::state::{self, Layout};
use super::{Opts, UpdateReport};
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
    let (todo, mut dirty) = scan::pending(scan::list_files(&opts.roots), &mut index);
    let mut fresh = Vec::new();
    if !todo.is_empty() {
        let mut seen: HashSet<String> = state::load_ids(&layout.rows, folded_before)?;
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
    if !old.is_empty() || oldest.is_some_and(|t| t + FOLD_SLACK_S < target) {
        report.folded = super::fold::fold(layout, target, &old, &mut index)?;
        dirty = true;
    }
    if dirty {
        state::save_index(&layout.index, &index)?;
    }
    report.updated = true;
    Ok(report)
}
