// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "test code")]

mod blame;
mod budget;
mod common;
mod ctx;
mod diff_log;
mod diff_moves;
mod files;
mod find;
mod follow;
mod guard;
mod map;
mod patch;
mod rev;
mod run_config_misc;
mod self_cmd;
mod show_outline;
