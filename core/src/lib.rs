// SPDX-License-Identifier: MPL-2.0
//! Owns the building blocks shared by the capsa tools: rendering one result as JSON, agent text
//! or a human view, the CLI skeleton, JSON-lines logs, the agent skill and Claude settings
//! hooks, hints, exit codes, program lookup, `~` expansion, config and state directories, the
//! census limit snapshot file, package managers and, with the `update` feature, self update.
//!
//! `capsa-core` depends on no workspace crate. It prints through [`render::emit`] and
//! `update::run`, and through [`exit`] for errors.

pub mod census_snapshot;
pub mod claude_settings;
#[cfg(feature = "clap")]
pub mod cli;
pub mod exit;
pub mod format;
pub mod hint;
pub mod log;
pub mod paths;
pub mod pkg;
pub mod render;
pub mod skill;
#[cfg(feature = "update")]
pub mod update;
pub mod which;
