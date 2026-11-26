// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "test code")]

mod batch;
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
mod permission_mode;
mod rev;
mod run;
mod run_config_misc;
mod run_corpus;
#[cfg(feature = "script")]
mod script;
#[cfg(feature = "script")]
mod script_edit;
#[cfg(feature = "script")]
mod script_host;
#[cfg(feature = "script")]
mod script_mcp;
#[cfg(feature = "script")]
mod script_output;
#[cfg(feature = "script")]
mod script_permissions;
#[cfg(feature = "script")]
mod script_saved;
mod self_cmd;
mod show_outline;
