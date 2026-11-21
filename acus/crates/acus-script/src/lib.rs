// SPDX-License-Identifier: MPL-2.0
//! Runs a JavaScript program on an embedded QuickJS engine, limited in time and memory, with only
//! the functions a [`Host`] names and `print` in reach.

mod engine;
mod error;
mod host;
mod imports;
mod limits;
mod module;
mod pool;

pub use engine::{Outcome, run};
pub use error::ScriptError;
pub use host::{Host, HostError};
pub use limits::Limits;
pub use module::{ModuleInfo, check_module, run_module};

#[cfg(test)]
mod tests;
