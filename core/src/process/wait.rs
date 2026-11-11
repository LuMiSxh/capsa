// SPDX-License-Identifier: MPL-2.0
//! Async waiting for termination signals and for a process group to vanish.

use std::time::Duration;

use tokio::time::{Instant, sleep};

use super::{ProcessError, Signal, group_alive};

/// Termination requests a proxy forwards to its child. SIGTERM, SIGHUP and SIGINT on Unix;
/// Windows has only Ctrl+C, reported as [`Signal::Int`].
pub struct Signals {
    #[cfg(unix)]
    term: tokio::signal::unix::Signal,
    #[cfg(unix)]
    hup: tokio::signal::unix::Signal,
    #[cfg(unix)]
    int: tokio::signal::unix::Signal,
}

impl Signals {
    /// Installs the handlers; call it inside a tokio runtime.
    ///
    /// # Errors
    ///
    /// [`ProcessError::Handler`] when the system refuses a handler.
    pub fn new() -> Result<Self, ProcessError> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let handler = |kind| signal(kind).map_err(ProcessError::Handler);
            Ok(Self {
                term: handler(SignalKind::terminate())?,
                hup: handler(SignalKind::hangup())?,
                int: handler(SignalKind::interrupt())?,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    /// Completes with the next request received.
    pub async fn recv(&mut self) -> Signal {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = self.term.recv() => Signal::Term,
                _ = self.hup.recv() => Signal::Hup,
                _ = self.int.recv() => Signal::Int,
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            Signal::Int
        }
    }
}

/// Completes on SIGINT, and on SIGTERM where the platform has it. Falls back to SIGINT alone
/// when the SIGTERM handler cannot be installed.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Polls every 25 ms until no member of the process group is left or `limit` passes. Returns
/// whether the group is gone.
pub async fn wait_group_gone(pgid: u32, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while group_alive(pgid) {
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(25)).await;
    }
    true
}
