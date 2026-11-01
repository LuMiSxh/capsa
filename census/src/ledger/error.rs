// SPDX-License-Identifier: MPL-2.0
//! Lists what can go wrong while reading or updating the ledger.

use capsa_core::census_snapshot::SnapshotError;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("cannot access {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot encode a ledger line")]
    Encode(#[from] serde_json::Error),
    #[error("another census process holds the ledger lock")]
    Busy,
    #[error(transparent)]
    Snapshots(#[from] SnapshotError),
}
