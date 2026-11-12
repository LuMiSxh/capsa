// SPDX-License-Identifier: MPL-2.0
//! Publishes the DeepSeek gate to the capsa bulletin, where census reads it without a socket call.

use crate::gate::{Gate, unix_now};
use crate::refresh::LOG_LIMIT;
use capsa_core::bulletin::{self, BulletinError, Stamped};
use capsa_core::log::{self, Level};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::task::JoinHandle;

/// The bulletin name, which is also the file name `vicarius.json`.
pub const TOOL: &str = "vicarius";

/// The body of `vicarius.json`, which census reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bulletin {
    pub deepseek: Stamped<DeepSeek>,
}

/// The DeepSeek route as the gate holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeepSeek {
    /// Whether the route accepts requests.
    pub open: bool,
    /// The last balance in USD, or `None` when the account reports another currency.
    pub balance_usd: Option<f64>,
}

/// Returns how long a published entry stays current: two balance refresh periods, in seconds.
///
/// One late refresh does not make census treat the route as unknown.
pub fn ttl_secs(balance_refresh_minutes: u32) -> u64 {
    u64::from(balance_refresh_minutes) * 2 * 60
}

/// Writes the gate to the bulletin in the background, one write at a time.
#[derive(Clone)]
pub struct Publisher {
    dir: PathBuf,
    log_file: PathBuf,
    gate: Arc<Gate>,
    /// Held across the snapshot and the write, so the file ends with the newest gate state.
    order: Arc<Mutex<()>>,
}

impl Publisher {
    /// Returns a publisher that writes into `dir` and logs a failed write to `log_file`.
    pub fn new(dir: PathBuf, log_file: PathBuf, gate: Arc<Gate>) -> Self {
        Publisher {
            dir,
            log_file,
            gate,
            order: Arc::new(Mutex::new(())),
        }
    }

    /// Writes the gate on a blocking thread, so no task waits on the disk.
    ///
    /// A failed write is logged as a warning and otherwise ignored.
    pub fn spawn(&self, ttl: u64) -> JoinHandle<()> {
        let publisher = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = publisher.write(ttl) {
                publisher.log_failure(&error);
            }
        })
    }

    fn write(&self, ttl: u64) -> Result<(), BulletinError> {
        let _order = self.order.lock().unwrap_or_else(PoisonError::into_inner);
        let snap = self.gate.snapshot();
        let value = DeepSeek {
            open: snap.is_open(),
            balance_usd: snap
                .balance
                .filter(|_| snap.currency.as_deref() == Some("USD")),
        };
        let body = Bulletin {
            deepseek: Stamped {
                at: unix_now(),
                ttl,
                value,
            },
        };
        bulletin::write_in(&self.dir, TOOL, &body)
    }

    fn log_failure(&self, error: &BulletinError) {
        if let Some(dir) = self.log_file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let line = Failure {
            error: error.to_string(),
        };
        let _ = log::append(
            &self.log_file,
            LOG_LIMIT,
            Level::Warn,
            "bulletin.write",
            &line,
        );
    }
}

#[derive(Serialize)]
struct Failure {
    error: String,
}

/// Removes the vicarius bulletin from `dir`, for `self uninstall`.
///
/// # Errors
/// Fails when the bulletin exists and cannot be removed.
pub fn withdraw(dir: &Path) -> Result<(), BulletinError> {
    bulletin::remove_in(dir, TOOL)
}
