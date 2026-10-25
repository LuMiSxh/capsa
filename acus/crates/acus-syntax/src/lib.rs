// SPDX-License-Identifier: MPL-2.0
//! Owns language detection, tree-sitter outlines and the addresses that chain onto them.
mod address;
pub mod calls;
mod dialect;
mod lang;
mod outline;
mod rules;

pub use address::{Address, Resolve, Target, resolve};
pub use dialect::is_jsonc;
pub use lang::Lang;
pub use outline::{Outline, Symbol, attached_start, outline, syntax_errors};
pub use rules::KINDS;
