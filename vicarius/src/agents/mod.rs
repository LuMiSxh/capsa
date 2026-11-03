// SPDX-License-Identifier: MPL-2.0
//! Derives the DeepSeek variants of configured agents and keeps their files in step.

mod frontmatter;
mod sync;
#[cfg(test)]
mod tests;

pub use frontmatter::{DeriveError, derive, is_derived};
pub use sync::{Entry, Outcome, SyncReport, sync};
