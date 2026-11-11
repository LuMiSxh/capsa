// SPDX-License-Identifier: MPL-2.0
//! Owns the checks every `self doctor` shares: the check constructors, the probes for directories
//! and files, the PATH check and the health of a JSON-lines log.

mod log_health;
mod on_path;
mod probe;
#[cfg(test)]
mod tests;

use std::borrow::Cow;

use crate::render::{Check, Status};

pub use log_health::log_checks;
pub use on_path::path_check;
pub use probe::{dir_check, same_file, writable};

/// Builds a check with its name, status, what it found and, when it failed, the fix.
pub fn check(
    name: impl Into<Cow<'static, str>>,
    status: Status,
    detail: impl Into<String>,
    hint: Option<&str>,
) -> Check {
    Check {
        name: name.into(),
        status,
        detail: detail.into(),
        hint: hint.map(str::to_owned),
    }
}

/// Builds an `Ok` check, which has no fix.
pub fn ok(name: impl Into<Cow<'static, str>>, detail: impl Into<String>) -> Check {
    check(name, Status::Ok, detail, None)
}
