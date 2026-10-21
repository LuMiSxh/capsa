// SPDX-License-Identifier: MPL-2.0
//! Owns the building blocks shared by the capsa tools: hints, output formats,
//! exit codes, program lookup, `~` expansion, package managers and, with the
//! `update` feature, self update.
//!
//! `capsa-core` depends on no workspace crate. It prints only through [`exit`],
//! whose job is rendering errors.

pub mod exit;
pub mod format;
pub mod hint;
pub mod paths;
pub mod pkg;
#[cfg(feature = "update")]
pub mod update;
pub mod which;
