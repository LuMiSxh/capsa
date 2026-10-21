// SPDX-License-Identifier: MPL-2.0
//! `acus self uninstall`: removes the skill acus installed, then the binary.

use anyhow::Result;
use capsa_core::exit::Outcome;

#[derive(clap::Args)]
pub struct Args {
    /// Print what would be removed and remove nothing.
    #[arg(long)]
    dry_run: bool,
}

#[cfg(not(feature = "cmd-update"))]
pub fn run(_: &Args) -> Result<Outcome> {
    Err(capsa_core::hint::Hinted::new(
        "acus was built without `uninstall`",
        "remove the binary by hand, or rebuild with --features cmd-update",
    )
    .into())
}

/// Removes the skill files first, so no skill outlives the binary it describes. The binary is
/// checked beforehand: a refusal there leaves the skill in place.
#[cfg(feature = "cmd-update")]
pub fn run(a: &Args) -> Result<Outcome> {
    use anyhow::Context;
    use capsa_core::hint::ResultExt;
    use capsa_core::update::uninstall;

    let home = std::env::home_dir().context("no home directory")?;
    uninstall(&crate::TOOL, true).hinted()?;
    for file in crate::cmd_skill::uninstall(&home, a.dry_run)? {
        println!("{} {}", verb(a.dry_run), file.display());
    }
    println!("{}", uninstall(&crate::TOOL, a.dry_run).hinted()?);
    if !a.dry_run {
        println!("hooks in ~/.claude/settings.json are left; remove the acus entries by hand");
    }
    Ok(Outcome::Found)
}

#[cfg(feature = "cmd-update")]
fn verb(dry_run: bool) -> &'static str {
    if dry_run { "would remove" } else { "removed" }
}
