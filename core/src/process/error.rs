// SPDX-License-Identifier: MPL-2.0
//! Errors of the process module.

/// Why a signal could not be sent or a signal handler not installed.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// The id is 0 or 1, or does not fit the platform's pid type; signalling it could reach
    /// the caller's own group or every process.
    #[error("{0} is not a valid process group id")]
    InvalidGroup(u32),
    /// The operating system refused the signal, for example because the group is gone.
    #[error("could not signal process group {pgid}")]
    Signal {
        /// The group that was addressed.
        pgid: u32,
        /// The operating system's answer.
        #[source]
        source: std::io::Error,
    },
    /// A signal handler could not be installed.
    #[error("could not install a signal handler")]
    Handler(#[source] std::io::Error),
}
