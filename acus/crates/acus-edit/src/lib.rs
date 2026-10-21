// SPDX-License-Identifier: MPL-2.0
//! Patches with `*** ` directives, SEARCH/REPLACE blocks and `*** Replace Symbol: path#Sym`,
//! applied all-or-nothing with atomic writes.
mod apply;
mod error;
mod parse;

pub use apply::{Applied, Change, FileText, Region, apply, apply_with};
pub use error::EditError;
pub use parse::{Chunk, InsertAt, Op, parse};
