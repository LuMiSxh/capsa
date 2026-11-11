// SPDX-License-Identifier: MPL-2.0
//! Compares where a tool resolves on PATH with the binary its hooks run.

use std::path::Path;

use super::{check, ok, same_file};
use crate::render::{Check, Status};

/// Builds the `PATH` check of `tool`: `found` is where the lookup on PATH resolved it, and
/// `expected` is the binary the hooks run.
///
/// A copy on PATH that is another file than `expected` warns, and so does a tool that is not on
/// PATH. With `expected` unknown, any copy on PATH passes.
pub fn path_check(tool: &str, found: Option<&Path>, expected: Option<&Path>) -> Check {
    match (found, expected) {
        (Some(found), Some(expected)) if !same_file(found, expected) => {
            let hint =
                format!("the hooks run the {tool} on PATH; remove the other copy or update it");
            check(
                "PATH",
                Status::Warn,
                format!("{tool} on PATH is {}", found.display()),
                Some(hint.as_str()),
            )
        }
        (Some(found), _) => ok("PATH", found.display().to_string()),
        (None, _) => {
            let hint =
                format!("the hooks run bare `{tool}`; add the directory of this binary to PATH");
            check(
                "PATH",
                Status::Warn,
                format!("{tool} is not on PATH"),
                Some(hint.as_str()),
            )
        }
    }
}
