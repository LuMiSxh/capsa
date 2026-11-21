// SPDX-License-Identifier: MPL-2.0
//! The resource limits of one script run.

use std::time::Duration;

/// Bounds the time, memory and stack of one run.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Wall-clock time the whole run may take. JavaScript is interrupted at it, queued calls never
    /// start, and running calls get the deadline to end by and a 2 s grace after it.
    pub deadline: Duration,
    /// Bytes the engine may allocate.
    pub memory: usize,
    /// Bytes of native stack the engine may use.
    pub stack: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            deadline: Duration::from_secs(120),
            memory: 256 << 20,
            stack: 1 << 20,
        }
    }
}
