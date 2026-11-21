// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "test code")]

mod blame;
mod budget;
mod bulletin;
mod common;
mod diff_log;
mod diff_moves;
mod diff_status;
mod files;
mod find;
mod follow;
mod guard;
mod map;
mod patch;
mod rev;
mod run;
mod run_config_misc;
#[cfg(feature = "script")]
mod script;
#[cfg(feature = "script")]
mod script_mcp;
#[cfg(feature = "script")]
mod script_saved;
mod self_cmd;
mod show_outline;
