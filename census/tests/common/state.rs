// SPDX-License-Identifier: MPL-2.0
//! Relative paths of tool state under the scratch home that the census tests build.

/// Returns the relative path of `name` in `tool`'s state directory under the scratch root
/// `state`, laid out as `capsa_core::paths::state_dir` lays it out on this platform.
pub fn tool_state(tool: &str, name: &str) -> String {
    if cfg!(windows) {
        format!("state/{tool}/state/{name}")
    } else {
        format!("state/{tool}/{name}")
    }
}
