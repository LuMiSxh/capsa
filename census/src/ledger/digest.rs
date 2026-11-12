// SPDX-License-Identifier: MPL-2.0
//! Keeps `digest.json`, the list-price USD of each raw request and hourly aggregate. The limits
//! read it instead of the ledger files. The updater keeps it in step with `rows.jsonl` and
//! `hourly.jsonl`. A digest that records other file sizes, another census version or another price
//! list is stale. The rows file is identified by its length and a hash of its last bytes, so a
//! file that was cut or replaced is noticed even when it has the same length.

use super::error::LedgerError;
use super::model::{Hourly, Row};
use super::state::{self, Layout};
use crate::usage::Tokens;
use crate::usage::prices;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, LedgerError>;

/// Layout of `digest.json`. Bump it when a digest with the same fields would mean something else.
const FORMAT: u32 = 3;

/// Bytes at the end of `rows.jsonl` that the digest hashes. More than one row, so a replaced file
/// with the same length still differs.
const TAIL_BYTES: u64 = 256;

/// The list-price USD of each raw request and each hourly aggregate, in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Priced {
    /// Request time and USD of each raw request that counts toward the limits.
    pub raw: Vec<(u64, f64)>,
    /// Hour start and USD of each hourly aggregate.
    pub hourly: Vec<(u64, f64)>,
}

impl Priced {
    /// Prices the limit requests and the aggregates. A model without a price counts as 0 USD, and
    /// requests of other providers are left out.
    pub fn of(rows: &[Row], hourly: &[Hourly]) -> Self {
        Priced {
            raw: rows
                .iter()
                .filter_map(|r| Some((r.t, limit_usd(r)?)))
                .collect(),
            hourly: hourly.iter().map(|h| (h.hour, aggregate_usd(h))).collect(),
        }
    }

    /// True when there are no raw rows and no aggregates.
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty() && self.hourly.is_empty()
    }
}

/// The digest as written. `rows_len` and `hourly_len` are the sizes of the ledger files it was
/// built from or appended to, and `census` is the version whose prices the USD values use.
#[derive(Serialize, Deserialize)]
pub(super) struct Digest {
    /// The layout the digest was written in, see `FORMAT`.
    format: u32,
    census: String,
    /// Unix seconds of the last write.
    updated: u64,
    rows_len: u64,
    /// FNV-1a hash of the last `TAIL_BYTES` bytes of `rows.jsonl` at `rows_len`.
    rows_tail: u64,
    hourly_len: u64,
    /// Message id, time and USD of each raw row, the USD absent for a request billed to another
    /// provider. The ids let the updater skip requests it has read.
    raw: Vec<(String, u64, Option<f64>)>,
    hourly: Vec<(u64, f64)>,
    /// The fetch time of the price list the USD values were priced with.
    prices: u64,
}

impl Digest {
    /// Returns the message ids of the raw rows.
    pub(super) fn ids(&self) -> HashSet<String> {
        self.raw.iter().map(|(id, _, _)| id.clone()).collect()
    }

    fn rows_key(&self) -> RowsKey {
        RowsKey {
            len: self.rows_len,
            tail: self.rows_tail,
        }
    }

    pub(super) fn to_priced(&self) -> Priced {
        Priced {
            raw: self
                .raw
                .iter()
                .filter_map(|(_, t, usd)| Some((*t, (*usd)?)))
                .collect(),
            hourly: self.hourly.clone(),
        }
    }

    /// Adds the rows the updater appended to `rows.jsonl`, which now ends in `key`.
    pub(super) fn append(&mut self, rows: &[Row], key: RowsKey, now: u64) {
        self.raw
            .extend(rows.iter().map(|r| (r.id.clone(), r.t, limit_usd(r))));
        self.rows_len = key.len;
        self.rows_tail = key.tail;
        self.updated = now;
    }
}

/// Identifies one state of `rows.jsonl`: its length and the hash of its last bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RowsKey {
    len: u64,
    tail: u64,
}

impl RowsKey {
    /// Reads the key of `path`; a missing file has length 0. The length is read first and the hash
    /// covers only bytes below it, so a file that grows meanwhile gives an older, consistent key.
    pub(super) fn of(path: &Path) -> Self {
        let empty = RowsKey { len: 0, tail: 0 };
        let Ok(mut file) = File::open(path) else {
            return empty;
        };
        let Ok(len) = file.metadata().map(|m| m.len()) else {
            return empty;
        };
        let start = len.saturating_sub(TAIL_BYTES);
        let mut tail = Vec::new();
        if file.seek(SeekFrom::Start(start)).is_err()
            || file.take(len - start).read_to_end(&mut tail).is_err()
        {
            return empty;
        }
        let hash = tail.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
        });
        RowsKey { len, tail: hash }
    }
}

/// Returns the size of `path`, or 0 when it is missing.
pub(super) fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// Returns the current unix time in seconds.
pub(super) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Returns the digest when this census version wrote it for the ledger files as they are now.
pub(super) fn read(layout: &Layout) -> Option<Digest> {
    let digest: Digest = serde_json::from_slice(&std::fs::read(&layout.digest).ok()?).ok()?;
    let current = digest.format == FORMAT
        && digest.census == env!("CARGO_PKG_VERSION")
        && digest.rows_key() == RowsKey::of(&layout.rows)
        && digest.hourly_len == file_len(&layout.hourly)
        && digest.prices == prices::fetched_at();
    current.then_some(digest)
}

/// Builds the digest from the ledger files. The keys are read first, so a file that grows
/// meanwhile makes the digest stale rather than wrong.
pub(super) fn build(layout: &Layout, now: u64) -> Result<Digest> {
    let (rows_key, hourly_len) = (RowsKey::of(&layout.rows), file_len(&layout.hourly));
    let (folded, hourly) = state::load_hourly(&layout.hourly)?;
    let rows = state::load_rows(&layout.rows, folded)?;
    Ok(Digest {
        format: FORMAT,
        census: env!("CARGO_PKG_VERSION").to_owned(),
        updated: now,
        rows_len: rows_key.len,
        rows_tail: rows_key.tail,
        hourly_len,
        raw: rows
            .into_iter()
            .map(|r| {
                let usd = limit_usd(&r);
                (r.id, r.t, usd)
            })
            .collect(),
        hourly: hourly.iter().map(|h| (h.hour, aggregate_usd(h))).collect(),
        prices: prices::fetched_at(),
    })
}

/// Replaces `digest.json` with `digest`. The file is a cache, so it is renamed into place without
/// the syncs that make a crash keep it; a torn or missing file is built again.
pub(super) fn write(layout: &Layout, digest: &Digest) -> Result<()> {
    state::write_cache(&layout.digest, &serde_json::to_vec(digest)?)
}

/// Returns the USD of a request that counts toward the limits, or `None` for any other provider.
/// The limits and the budget month read only `anthropic`, whatever `billing` in the config says,
/// because the windows track Claude's own limits.
fn limit_usd(r: &Row) -> Option<f64> {
    prices::counts_toward_limits(&r.model).then(|| row_usd(r))
}

fn row_usd(r: &Row) -> f64 {
    let t = Tokens {
        input: r.input,
        cache_read: r.cache_read,
        cache_write_5m: r.cache_write_5m,
        cache_write_1h: r.cache_write_1h,
        output: r.output,
        requests: 1,
    };
    prices::request_cost(&r.model, &t).unwrap_or(0.0)
}

fn aggregate_usd(h: &Hourly) -> f64 {
    let t = &h.tokens;
    let t = Tokens {
        input: t.input,
        cache_read: t.cache_read,
        cache_write_5m: t.cache_write_5m,
        cache_write_1h: t.cache_write_1h,
        output: t.output,
        requests: t.requests,
    };
    prices::family_cost(&h.family, &t).unwrap_or(0.0)
}
