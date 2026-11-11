// SPDX-License-Identifier: MPL-2.0
//! Owns the errors `self update` and `self uninstall` return in a build without the `cmd-update`
//! feature. They live here rather than in `update`, which is compiled only with `update`.

use super::Hinted;

/// Returns the error that `<tool> self update` gives without `cmd-update`. The hint installs the
/// tool from `package_path`, its crate directory in the workspace.
pub fn update_unavailable(tool: &str, package_path: &str) -> anyhow::Error {
    Hinted::new(
        format!("{tool} was built without `cmd-update`"),
        format!(
            "cargo install --path {package_path} --features cmd-update, or rerun the installer"
        ),
    )
    .into()
}

/// Returns the error that `<tool> self uninstall` gives without `cmd-update`.
pub fn uninstall_unavailable(tool: &str) -> anyhow::Error {
    Hinted::new(
        format!("{tool} was built without `cmd-update`"),
        "remove the binary by hand, or rebuild with --features cmd-update",
    )
    .into()
}
