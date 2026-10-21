// SPDX-License-Identifier: MPL-2.0
//! The clap arguments of `<tool> self update` and `<tool> self uninstall`, shared by the tools.

use super::Mode;

/// Arguments of `self update`.
#[derive(Debug, Clone, Copy, clap::Args)]
pub struct UpdateArgs {
    /// Only report whether a newer release exists (exit 0 if so, 1 if current).
    #[arg(long)]
    pub check: bool,
    /// Reinstall the latest release even when it is not newer.
    #[arg(long, conflicts_with = "check")]
    pub force: bool,
}

impl UpdateArgs {
    /// The update mode these flags select.
    pub fn mode(self) -> Mode {
        match (self.check, self.force) {
            (true, _) => Mode::Check,
            (false, true) => Mode::Force,
            (false, false) => Mode::Install,
        }
    }
}

/// Arguments of `self uninstall`.
#[derive(Debug, Clone, Copy, clap::Args)]
pub struct UninstallArgs {
    /// Print what would be removed and remove nothing.
    #[arg(long)]
    pub dry_run: bool,
}
