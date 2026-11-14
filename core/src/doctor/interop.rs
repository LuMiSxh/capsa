// SPDX-License-Identifier: MPL-2.0
//! Builds the `interop` doctor group: whether each sibling tool is installed, how old its bulletin
//! is, and which features that depend on it are active. Every line is informational.

use super::ok;
use crate::bulletin;
use crate::format::ago;
use crate::render::Group;
use crate::which;

/// A sibling tool that the calling tool uses when the sibling is installed.
#[derive(Debug, Clone, Copy)]
pub struct Sibling<'a> {
    /// The binary name of the sibling, which names its line in the group.
    pub tool: &'a str,
    /// One short phrase saying what uses the sibling, such as
    /// `guard blocks in the census status line`.
    pub feature: &'a str,
    /// The bulletin whose age the line reports, when the sibling publishes one.
    pub bulletin: Option<&'a str>,
}

/// Returns the `interop` group for `siblings`, read from PATH and the capsa bulletin directory.
///
/// A sibling that is not installed is off, which is not a problem, so every line is ok.
pub fn interop_group(siblings: &[Sibling<'_>]) -> Group {
    interop_group_with(
        siblings,
        |tool| which::find(tool).is_some(),
        bulletin::written_at,
        bulletin::now(),
    )
}

/// Builds the group from the `installed` and `written_at` lookups, with `now` in unix seconds.
///
/// `written_at` returns the unix second a bulletin was last written, or `None` when it is missing.
pub(super) fn interop_group_with(
    siblings: &[Sibling<'_>],
    installed: impl Fn(&str) -> bool,
    written_at: impl Fn(&str) -> Option<u64>,
    now: u64,
) -> Group {
    let checks = siblings
        .iter()
        .map(|sibling| {
            let line = if installed(sibling.tool) {
                let state = bulletin_state(sibling, &written_at, now);
                format!("{state}, active: {}", sibling.feature)
            } else {
                format!("not installed, off: {}", sibling.feature)
            };
            ok(sibling.tool.to_owned(), line)
        })
        .collect();
    Group {
        name: "interop",
        checks,
    }
}

fn bulletin_state(
    sibling: &Sibling<'_>,
    written_at: &impl Fn(&str) -> Option<u64>,
    now: u64,
) -> String {
    match sibling.bulletin {
        None => "installed".to_owned(),
        Some(name) => match written_at(name) {
            Some(at) => format!("installed, bulletin {}", ago(now.saturating_sub(at))),
            None => "installed, no bulletin".to_owned(),
        },
    }
}
