// SPDX-License-Identifier: MPL-2.0
//! `acus self update`: replaces this binary with the latest release, then reinstalls the skill.

use anyhow::Result;
use capsa_core::exit::Outcome;

#[cfg(feature = "cmd-update")]
pub use capsa_core::update::UpdateArgs as Args;

#[cfg(not(feature = "cmd-update"))]
#[derive(clap::Args)]
pub struct Args {}

#[cfg(not(feature = "cmd-update"))]
pub fn run(_: Args) -> Result<Outcome> {
    Err(capsa_core::hint::Hinted::new(
        "acus was built without `update`",
        "cargo install --path crates/acus-cli --features cmd-update, or rerun the installer",
    )
    .into())
}

#[cfg(feature = "cmd-update")]
pub fn run(a: Args) -> Result<Outcome> {
    use crate::agents::{Agent, installed_agent};
    use capsa_core::hint::{Hinted, ResultExt};
    use std::process::Command;

    let report = capsa_core::update::update(&crate::TOOL, a.mode()).hinted()?;
    println!("{report}");
    if a.check {
        return Ok(report.outcome(true));
    }
    // The binary on disk carries the skill of its version; reinstall it where it already is.
    let agent = std::env::home_dir().map_or(Agent::Claude, |h| installed_agent(&h));
    let ok = Command::new(&report.exe)
        .args(["self", "skill", "--install", "--agent", agent.flag()])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err(Hinted::new(
            "cannot reinstall the skill",
            "run acus self skill --install",
        )
        .into());
    }
    Ok(Outcome::Found)
}
