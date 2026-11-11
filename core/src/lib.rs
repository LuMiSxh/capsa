// SPDX-License-Identifier: MPL-2.0
//! Owns the building blocks shared by the capsa tools: rendering one result as JSON, agent text
//! or a human view, the CLI skeleton, JSON-lines logs, the agent skill, Claude settings, the
//! hooks a tool registers with an agent, hints, exit codes, program lookup, `~` expansion, config
//! and state directories, the
//! census limit snapshot file, package managers and, with the `update` feature, self update.
//!
//! `capsa-core` depends on no workspace crate. It prints through [`render::emit`] and
//! `update::run`, and through [`exit`] for errors.

pub mod bulletin;
pub mod census_snapshot;
pub mod claude_settings;
#[cfg(feature = "clap")]
pub mod cli;
#[cfg(feature = "config")]
pub mod config;
pub mod doctor;
pub mod exit;
pub mod format;
pub mod fs;
pub mod hint;
pub mod hooks;
pub mod lock;
pub mod log;
pub mod paths;
pub mod pkg;
#[cfg(feature = "process")]
pub mod process;
pub mod render;
pub mod skill;
#[cfg(feature = "update")]
pub mod update;
pub mod which;
