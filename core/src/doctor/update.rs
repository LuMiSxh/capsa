// SPDX-License-Identifier: MPL-2.0
//! Builds the doctor line that says whether a newer release of a tool exists.

use crate::render::{Check, Status};
use crate::update::{Tool, UpdateCheck};

use super::{check, ok};

/// Returns the update line of `tool`'s doctor report: ok when `result` is the newest release, a
/// warning that names `<tool> self update` when a newer one exists, and ok when GitHub did not
/// answer.
pub fn update_check(tool: &Tool, result: &UpdateCheck) -> Check {
    match result {
        UpdateCheck::Current => ok("update", format!("{} is the latest", tool.version)),
        UpdateCheck::Available(latest) => {
            let hint = format!("run {} self update", tool.name);
            check(
                "update",
                Status::Warn,
                format!("{latest} available (running {})", tool.version),
                Some(hint.as_str()),
            )
        }
        UpdateCheck::Unknown => ok("update", "could not check for updates"),
    }
}
