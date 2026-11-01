// SPDX-License-Identifier: MPL-2.0
//! Lists what can go wrong when the limits read the ledger and the snapshot file.

use crate::ledger::LedgerError;
use capsa_core::census_snapshot::SnapshotError;

/// Lists the failures of [`status`](super::status) and [`usage_by`](super::usage_by).
#[derive(Debug, thiserror::Error)]
pub enum LimitsError {
    #[error("cannot read the usage ledger")]
    Ledger(#[from] LedgerError),
    #[error("cannot read the limit snapshots")]
    Snapshots(#[from] SnapshotError),
}
