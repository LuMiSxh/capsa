// SPDX-License-Identifier: MPL-2.0
//! Transcript analytics for Claude Code and Codex, behind the `census` CLI.

pub mod config;
pub mod context;
pub mod hook;
pub mod interop;
pub mod ledger;
pub mod limits;
pub mod session;
pub mod statusline;
mod transcript;
pub mod usage;

/// The label for a value that a transcript or the price table does not name.
pub const UNKNOWN: &str = "unknown";
