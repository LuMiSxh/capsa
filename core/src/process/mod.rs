// SPDX-License-Identifier: MPL-2.0
//! Owns process-group teardown and, with `process-async`, termination-signal waiting.
//!
//! A child started with [`own_group`] leads a process group whose id is the child's pid, so
//! [`signal_group`] reaches the child and everything it spawned. Windows has no process groups
//! or graceful requests: every signal ends the process tree through `taskkill /T /F`, on a
//! best-effort basis.

mod error;
mod group;
#[cfg(feature = "process-async")]
mod wait;

pub use error::ProcessError;
pub use group::{Signal, exit_code, group_alive, own_group, signal_group};
#[cfg(feature = "process-async")]
pub use wait::{Signals, shutdown_signal, wait_group_gone};

#[cfg(test)]
mod tests;
