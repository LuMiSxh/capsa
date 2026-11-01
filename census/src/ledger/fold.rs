// SPDX-License-Identifier: MPL-2.0
//! Folds aged raw rows into hourly aggregates and prunes the aggregates.

use super::error::LedgerError;
use super::model::{Hourly, Row, Tokens, family_of, floor_hour};
use super::state::{self, Index, Layout};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, LedgerError>;
type Buckets = BTreeMap<(u64, String, bool), Tokens>;

fn add_rows(buckets: &mut Buckets, rows: &[Row]) {
    for r in rows {
        let key = (
            floor_hour(r.t),
            family_of(&r.model).to_owned(),
            r.agent.is_some(),
        );
        buckets.entry(key).or_default().add_row(r);
    }
}

/// Moves every raw row before `before` (an hour boundary), plus the not yet stored `extra` rows,
/// into the hourly file, then rewrites the raw rows without them.
///
/// The hourly file records `before` as its folded-before time first, so a crash before the raw
/// rewrite leaves rows that readers already ignore and the next fold drops.
pub(super) fn fold(
    layout: &Layout,
    before: u64,
    extra: &[Row],
    index: &mut Index,
) -> Result<usize> {
    let (head, hourly) = state::load_hourly(&layout.hourly)?;
    let rows = state::load_rows(&layout.rows, head)?;
    let (old, keep): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|r| r.t < before);
    let mut buckets: Buckets = hourly
        .into_iter()
        .map(|h| ((h.hour, h.family, h.sub), h.tokens))
        .collect();
    add_rows(&mut buckets, &old);
    add_rows(&mut buckets, extra);
    let out: Vec<Hourly> = buckets
        .into_iter()
        .map(|((hour, family, sub), tokens)| Hourly {
            hour,
            family,
            sub,
            tokens,
        })
        .collect();
    state::save_hourly(&layout.hourly, before.max(head), &out)?;
    if !old.is_empty() {
        state::rewrite_rows(&layout.rows, &keep)?;
    }
    index.oldest_t = keep.iter().map(|r| r.t).min();
    Ok(old.len() + extra.len())
}

/// Counts what a prune removed, or would remove.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub hourly_removed: usize,
    pub snapshots_removed: usize,
    pub dry_run: bool,
}

/// Removes hourly aggregates and snapshots older than `before`; raw rows are never touched.
pub(super) fn prune(layout: &Layout, before: u64, dry_run: bool) -> Result<PruneReport> {
    let (head, hourly) = state::load_hourly(&layout.hourly)?;
    let (keep, drop): (Vec<Hourly>, Vec<Hourly>) =
        hourly.into_iter().partition(|h| h.hour >= before);
    if !dry_run && !drop.is_empty() {
        state::save_hourly(&layout.hourly, head, &keep)?;
    }
    let snapshots_removed =
        capsa_core::census_snapshot::prune_before(&layout.snapshots, before, dry_run)?;
    Ok(PruneReport {
        hourly_removed: drop.len(),
        snapshots_removed,
        dry_run,
    })
}
