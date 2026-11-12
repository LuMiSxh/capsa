// SPDX-License-Identifier: MPL-2.0
//! Routes Claude Code subagents to DeepSeek while credit lasts, behind the `vicarius` CLI.

use std::path::PathBuf;

pub mod agents;
pub mod balance;
pub mod config;
pub mod gate;
pub mod proxy;
pub mod refresh;

/// Returns where the proxy writes `proxy.jsonl`: the vicarius state directory.
pub fn proxy_log_file() -> PathBuf {
    capsa_core::paths::log_file("vicarius", "proxy.jsonl")
        .unwrap_or_else(|| capsa_core::paths::state_dir("vicarius").join("proxy.jsonl"))
}
